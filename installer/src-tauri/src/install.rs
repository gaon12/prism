//! The install and uninstall steps, independent of the window and of Windows itself.
//!
//! Everything that touches the operating system (registry, shortcuts, known folders) goes through
//! the [`System`] trait. The real implementation lives in `system.rs`; tests use a fake that only
//! records calls, so these steps can be checked without changing the computer.
//!
//! What an installation consists of:
//! - The files from the payload in the install folder (default `%LOCALAPPDATA%\Programs\Prism`).
//! - `uninstall.exe` (a copy of this setup) in the same folder.
//! - `install.json`, a record of exactly which files and shortcuts we created, so the uninstaller
//!   removes only those and never anything the user put there.
//! - Optional shortcuts (Start menu, desktop).
//! - The "Apps & features" entry `HKCU\...\Uninstall\Prism`. This is the same key the previous
//!   NSIS installer used, so installing over an NSIS installation simply takes it over.
//!
//! No administrator rights are needed: everything is per-user.

use crate::{payload, safe_path, transaction};
use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

pub const APP_NAME: &str = "Prism";
pub const APP_EXE: &str = "prism.exe";
pub const UNINSTALLER_EXE: &str = "uninstall.exe";
pub const RECORD_FILE: &str = "install.json";
/// Tauri stores app data in folders named after the app identifier.
pub const APP_IDENTIFIER: &str = "app.prism.desktop";

/// Folders that differ per user/computer.
#[derive(Clone, Debug)]
pub struct Folders {
    /// `%LOCALAPPDATA%`
    pub local_app_data: PathBuf,
    /// `%APPDATA%` (roaming)
    pub app_data: PathBuf,
    /// The user's desktop.
    pub desktop: PathBuf,
    /// `...\Start Menu\Programs`
    pub start_menu: PathBuf,
}

/// The "Apps & features" entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UninstallEntry {
    pub version: String,
    pub install_dir: PathBuf,
    /// Approximate size in KiB (Windows shows it in Settings).
    pub size_kib: u32,
}

/// Operating-system actions. Implementations must not log personal paths.
pub trait System {
    fn folders(&self) -> io::Result<Folders>;
    /// Opaque snapshot of the registration, captured before any installation mutation.
    fn capture_registration(&self) -> io::Result<Vec<u8>>;
    fn restore_registration(&self, snapshot: &[u8]) -> io::Result<()>;
    /// Creates (or replaces) a `.lnk` shortcut.
    fn create_shortcut(&self, link: &Path, target: &Path, description: &str) -> io::Result<()>;
    fn shortcut_points_to(&self, link: &Path, target: &Path) -> io::Result<bool>;
    fn write_uninstall_entry(&self, entry: &UninstallEntry) -> io::Result<()>;
    fn remove_uninstall_entry(&self) -> io::Result<()>;
    /// Install folder and version of an existing installation, if any.
    fn existing_install(&self) -> Option<(PathBuf, Option<String>)>;
}

/// Choices from the setup window (or defaults in silent mode).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InstallOptions {
    pub install_dir: PathBuf,
    pub desktop_shortcut: bool,
    pub start_menu_shortcut: bool,
}

/// What `install.json` contains.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InstallRecord {
    pub version: String,
    /// Paths relative to the install folder.
    pub files: Vec<String>,
    /// Absolute paths of shortcuts we created.
    pub shortcuts: Vec<PathBuf>,
}

/// Steps shown in the progress bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Stage {
    Preparing,
    Copying,
    Shortcuts,
    Registering,
    Removing,
    Done,
}

/// Default install folder: `%LOCALAPPDATA%\Programs\Prism` (no admin rights needed).
pub fn default_install_dir(folders: &Folders) -> PathBuf {
    folders.local_app_data.join("Programs").join(APP_NAME)
}

/// Checks that the chosen folder is usable before anything is written.
pub fn validate_install_dir(dir: &Path) -> Result<(), String> {
    if !dir.is_absolute() {
        return Err("Choose a full folder path, for example C:\\Programs\\Prism.".into());
    }
    safe_path::no_links(dir)
        .map_err(|_| "Choose a folder without symbolic links or junctions.".to_string())?;
    if dir.parent().is_none() {
        return Err("Prism cannot be installed directly in a drive root.".into());
    }
    if dir.is_file() {
        return Err("A file with this name already exists. Choose another folder.".into());
    }
    if dir.join("prism.portable").exists() {
        return Err(
            "This is a portable copy. Update its executable manually or choose another folder."
                .into(),
        );
    }
    // A non-empty folder that is not a Prism installation could contain the user's files;
    // installing there would mix them with ours.
    if let Ok(mut entries) = fs::read_dir(dir) {
        let has_files = entries.next().is_some();
        let is_prism = dir.join(APP_EXE).exists() || dir.join(RECORD_FILE).exists();
        if has_files && !is_prism {
            return Err("This folder is not empty. Choose an empty folder or a new one.".into());
        }
    }
    Ok(())
}

pub fn read_record(dir: &Path) -> InstallRecord {
    checked_record(dir).unwrap_or_default()
}

fn checked_record(dir: &Path) -> Result<InstallRecord, String> {
    use std::io::Read;
    let path = dir.join(RECORD_FILE);
    safe_path::no_links(&path)
        .map_err(|_| "The installation record path is unsafe.".to_string())?;
    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(InstallRecord::default())
        }
        Err(_) => return Err("Could not read the installation record.".into()),
    };
    let mut bytes = Vec::new();
    file.by_ref()
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Could not read the installation record.".to_string())?;
    if bytes.len() > 1024 * 1024 {
        return Err("The installation record is too large.".into());
    }
    let record: InstallRecord = serde_json::from_slice(&bytes)
        .map_err(|_| "The installation record is damaged; no files were removed.".to_string())?;
    if record.files.len() > 10_000 || record.shortcuts.len() > 2 || record.version.len() > 128 {
        return Err("The installation record exceeds allowed limits.".into());
    }
    for name in &record.files {
        safe_path::relative_name(name)
            .map_err(|_| "The installation record contains an unsafe file path.".to_string())?;
    }
    Ok(record)
}

fn checked_shortcuts(record: &InstallRecord, folders: &Folders) -> Result<Vec<PathBuf>, String> {
    record
        .shortcuts
        .iter()
        .map(|link| {
            safe_path::shortcut(link, &folders.desktop)
                .or_else(|_| safe_path::shortcut(link, &folders.start_menu))
                .map_err(|_| {
                    "The installation record contains an unrecognized shortcut.".to_string()
                })
        })
        .collect()
}

fn folder_size_kib(dir: &Path, files: &[String]) -> u32 {
    let bytes: u64 = files
        .iter()
        .filter_map(|name| fs::metadata(dir.join(name)).ok())
        .map(|meta| meta.len())
        .sum();
    (bytes / 1024).min(u32::MAX as u64) as u32
}

/// Additional free space needed while old files and staged replacements coexist.
pub fn required_free_bytes(
    dir: &Path,
    payload_bytes: u64,
    setup_bytes: u64,
) -> Result<u64, String> {
    let record = checked_record(dir)?;
    let mut names: std::collections::HashSet<String> = record.files.into_iter().collect();
    names.extend([
        APP_EXE.to_owned(),
        UNINSTALLER_EXE.to_owned(),
        RECORD_FILE.to_owned(),
    ]);
    let mut required = payload_bytes
        .checked_add(setup_bytes)
        .and_then(|bytes| bytes.checked_add(1024 * 1024))
        .ok_or("The installation size exceeds allowed limits.")?;
    if dir.exists() {
        for name in names {
            let path = safe_path::file(dir, &name)
                .map_err(|_| "An installation backup path is unsafe.".to_string())?;
            match fs::metadata(path) {
                Ok(metadata) => {
                    required = required
                        .checked_add(metadata.len())
                        .ok_or("The installation size exceeds allowed limits.")?
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(_) => return Err("Could not estimate the installation backup size.".into()),
            }
        }
    }
    Ok(required)
}

/// Installs (or updates) Prism.
///
/// `setup_exe` is the running setup file; it is copied as `uninstall.exe`.
/// `progress(stage, fraction)` is called with a fraction between 0.0 and 1.0.
pub fn install(
    system: &dyn System,
    payload_bytes: &[u8],
    version: &str,
    options: &InstallOptions,
    setup_exe: &Path,
    mut progress: impl FnMut(Stage, f64),
) -> Result<(), String> {
    let dir = &options.install_dir;
    progress(Stage::Preparing, 0.0);
    validate_install_dir(dir)?;
    fs::create_dir_all(dir).map_err(|error| format!("Could not create the folder: {error}"))?;
    let _lock = crate::folder_lock::FolderLock::acquire(dir).map_err(|error| error.to_string())?;
    let previous = checked_record(dir)?;
    let folders = system
        .folders()
        .map_err(|_| "Could not find the application folders.".to_string())?;
    checked_shortcuts(&previous, &folders)?;
    for folder in [&folders.desktop, &folders.start_menu] {
        let link = safe_path::file(folder, "Prism.lnk")
            .map_err(|_| "A shortcut path is unsafe.".to_string())?;
        if link.exists()
            && !system
                .shortcut_points_to(&link, &dir.join(APP_EXE))
                .unwrap_or(false)
        {
            return Err(
                "An existing Prism shortcut belongs to a different application; it was preserved."
                    .into(),
            );
        }
    }
    for name in &previous.files {
        safe_path::file(dir, name).map_err(|_| "A recorded file path is unsafe.".to_string())?;
    }

    let contents =
        payload::list(payload_bytes).map_err(|_| "The setup payload is damaged.".to_string())?;
    if !contents.iter().any(|file| file.name == APP_EXE)
        || contents.iter().any(|file| {
            file.name.eq_ignore_ascii_case(RECORD_FILE)
                || file.name.eq_ignore_ascii_case(UNINSTALLER_EXE)
                || file.name.to_ascii_lowercase().starts_with(".prism-")
        })
    {
        return Err("The setup does not contain a valid Prism application.".into());
    }
    for file in &contents {
        let target = safe_path::file(dir, &file.name)
            .map_err(|_| "A payload target is unsafe.".to_string())?;
        let owned = previous
            .files
            .iter()
            .any(|old| old.eq_ignore_ascii_case(&file.name))
            || (previous.files.is_empty() && file.name == APP_EXE);
        if target.exists() && !owned {
            return Err("A new application file would replace an unrecorded file. Move that file before updating.".into());
        }
    }
    let mut files: Vec<String> = contents.iter().map(|file| file.name.clone()).collect();
    files.push(UNINSTALLER_EXE.into());
    let registration = system
        .capture_registration()
        .map_err(|_| "Could not back up the application registration.".to_string())?;
    let mut transaction = transaction::Files::new(dir)
        .map_err(|_| "Could not prepare the installation backup.".to_string())?;
    for name in files
        .iter()
        .chain(&previous.files)
        .chain(std::iter::once(&RECORD_FILE.to_owned()))
    {
        transaction
            .snapshot(dir, name)
            .map_err(|_| "Could not back up an installation file.".to_string())?;
    }
    for folder in [&folders.desktop, &folders.start_menu] {
        transaction
            .snapshot(folder, "Prism.lnk")
            .map_err(|_| "Could not back up a shortcut.".to_string())?;
    }
    fs::write(
        transaction.directory.join("registration.json"),
        &registration,
    )
    .map_err(|_| "Could not save the registration recovery snapshot.".to_string())?;
    let mut registration_changed = false;
    let result = (|| -> Result<(), String> {
        // Extract everything into private staging before replacing the first installed file.
        progress(Stage::Copying, 0.05);
        let staged = transaction.stage();
        payload::extract(payload_bytes, &staged, |done, total| {
            let share = if total == 0 {
                1.0
            } else {
                done as f64 / total as f64
            };
            progress(Stage::Copying, 0.05 + 0.75 * share);
        })
        .map_err(|_| "Could not stage Prism's files.".to_string())?;
        fs::copy(setup_exe, staged.join(UNINSTALLER_EXE))
            .map_err(|_| "Could not stage the uninstaller.".to_string())?;
        transaction
            .begin_changes()
            .map_err(|_| "Could not save the file recovery map.".to_string())?;
        for name in &files {
            let target = safe_path::create_parent(dir, name)
                .map_err(|_| "An installation file path is unsafe.".to_string())?;
            let source = safe_path::file(&staged, name)
                .map_err(|_| "A staged file path is unsafe.".to_string())?;
            payload::replace_file(&source, &target).map_err(|_| {
                "Could not replace Prism's files. Close Prism and try again.".to_string()
            })?;
        }
        for old in previous
            .files
            .iter()
            .filter(|old| !files.iter().any(|name| name.eq_ignore_ascii_case(old)))
        {
            let path = safe_path::file(dir, old)
                .map_err(|_| "A recorded file path is unsafe.".to_string())?;
            match fs::remove_file(path) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => {
                    return Err("Could not remove an old application file.".into())
                }
                _ => {}
            }
        }
        progress(Stage::Shortcuts, 0.88);
        let app = dir.join(APP_EXE);
        let mut shortcuts = Vec::new();
        for (enabled, folder) in [
            (options.start_menu_shortcut, &folders.start_menu),
            (options.desktop_shortcut, &folders.desktop),
        ] {
            let link = safe_path::file(folder, "Prism.lnk")
                .map_err(|_| "A shortcut path is unsafe.".to_string())?;
            if enabled {
                // Remove the existing directory entry before the shell saves a new link,
                // so a hard link cannot turn Save into an overwrite of a different file.
                match fs::remove_file(&link) {
                    Err(error) if error.kind() != io::ErrorKind::NotFound => {
                        return Err("Could not replace a shortcut.".into())
                    }
                    _ => {}
                }
                system
                    .create_shortcut(&link, &app, "Search files, documents and clipboard")
                    .map_err(|_| "Could not create a shortcut.".to_string())?;
                shortcuts.push(folder.join("Prism.lnk"));
            } else if previous
                .shortcuts
                .iter()
                .any(|old| safe_path::shortcut(old, folder).is_ok())
            {
                match fs::remove_file(&link) {
                    Err(error) if error.kind() != io::ErrorKind::NotFound => {
                        return Err("Could not remove a shortcut.".into())
                    }
                    _ => {}
                }
            }
        }
        progress(Stage::Registering, 0.95);
        let record = InstallRecord {
            version: version.into(),
            files: files.clone(),
            shortcuts,
        };
        fs::write(
            staged.join(RECORD_FILE),
            serde_json::to_vec_pretty(&record)
                .map_err(|_| "Could not encode the install record.".to_string())?,
        )
        .map_err(|_| "Could not stage the install record.".to_string())?;
        let record_target = safe_path::file(dir, RECORD_FILE)
            .map_err(|_| "The install record path is unsafe.".to_string())?;
        payload::replace_file(&staged.join(RECORD_FILE), &record_target)
            .map_err(|_| "Could not save the install record.".to_string())?;
        registration_changed = true;
        system
            .write_uninstall_entry(&UninstallEntry {
                version: version.into(),
                install_dir: dir.clone(),
                size_kib: folder_size_kib(dir, &files),
            })
            .map_err(|_| "Could not register Prism with Windows.".to_string())?;
        Ok(())
    })();
    if let Err(message) = result {
        let files_restored = transaction.restore().is_ok();
        let registration_restored =
            !registration_changed || system.restore_registration(&registration).is_ok();
        if !files_restored || !registration_restored {
            transaction.retain();
            return Err(format!("{message} Recovery was incomplete; backup files were kept in the installation folder."));
        }
        transaction.finished();
        return Err(format!("{message} The previous installation was restored."));
    }
    transaction.finished();
    progress(Stage::Done, 1.0);
    Ok(())
}

/// Removes Prism. Only files listed in `install.json` (plus the record itself) are deleted, so
/// anything else the user stored in the folder is left alone. `remove_data` also deletes
/// settings, history and indexes in `%APPDATA%` / `%LOCALAPPDATA%`.
///
/// The running uninstaller cannot delete itself; the caller runs this from a temporary copy
/// (see `--from-temp` in `args.rs`).
pub fn uninstall(
    system: &dyn System,
    dir: &Path,
    remove_data: bool,
    mut progress: impl FnMut(Stage, f64),
) -> Result<(), String> {
    progress(Stage::Removing, 0.0);
    let dir = safe_path::root(dir).map_err(|_| "The installation folder is unsafe.".to_string())?;
    let dir = dir.as_path();
    let _lock = crate::folder_lock::FolderLock::acquire(dir).map_err(|error| error.to_string())?;
    let record = checked_record(dir)?;
    let folders = system
        .folders()
        .map_err(|_| "Could not find the application folders.".to_string())?;
    let shortcuts = checked_shortcuts(&record, &folders)?;
    for link in &shortcuts {
        if link.exists()
            && !system
                .shortcut_points_to(link, &dir.join(APP_EXE))
                .unwrap_or(false)
        {
            return Err(
                "A recorded shortcut no longer points to this installation; no files were removed."
                    .into(),
            );
        }
    }
    // A missing legacy record permits only known app files; damaged records fail above.
    let files = if record.files.is_empty() {
        vec![APP_EXE.to_string(), UNINSTALLER_EXE.to_string()]
    } else {
        record.files.clone()
    };
    // Validate the entire deletion plan before removing any file.
    for name in &files {
        safe_path::file(dir, name).map_err(|_| "A recorded file path is unsafe.".to_string())?;
    }
    let app = dir.join(APP_EXE);
    if app.exists() && fs::OpenOptions::new().write(true).open(&app).is_err() {
        return Err("Prism is still running. Close Prism and try again.".into());
    }
    let total = files.len().max(1) as f64;
    for (index, name) in files.iter().enumerate() {
        let path = safe_path::file(dir, name)
            .map_err(|_| "A recorded file path is unsafe.".to_string())?;
        if path.exists() {
            fs::remove_file(&path).map_err(|error| format!("Could not remove {name}: {error}"))?;
        }
        // Remove folders that became empty (e.g. "resources/").
        let mut parent = path.parent();
        while let Some(folder) = parent.filter(|folder| *folder != dir) {
            if fs::remove_dir(folder).is_err() {
                break;
            }
            parent = folder.parent();
        }
        progress(Stage::Removing, 0.8 * (index + 1) as f64 / total);
    }
    for link in &shortcuts {
        checked_shortcuts(&record, &folders)?;
        let _ = fs::remove_file(link);
    }
    let _ = fs::remove_file(dir.join(RECORD_FILE));
    // Deletes the folder only if nothing of the user's is left in it.
    let _ = fs::remove_dir(dir);
    if remove_data {
        if let Ok(folders) = system.folders() {
            for data in [
                folders.app_data.join(APP_IDENTIFIER),
                folders.local_app_data.join(APP_IDENTIFIER),
            ] {
                if data.exists() {
                    safe_path::no_links(&data)
                        .map_err(|_| "The application data folder is unsafe.".to_string())?;
                    let base = fs::canonicalize(data.parent().unwrap())
                        .map_err(|error| error.to_string())?;
                    let resolved = fs::canonicalize(&data).map_err(|error| error.to_string())?;
                    if resolved.parent() != Some(base.as_path()) {
                        return Err("The application data folder is unsafe.".into());
                    }
                    fs::remove_dir_all(&resolved)
                        .map_err(|error| format!("Could not remove Prism's data: {error}"))?;
                }
            }
        }
    }
    progress(Stage::Registering, 0.95);
    system
        .remove_uninstall_entry()
        .map_err(|error| format!("Could not unregister Prism: {error}"))?;
    progress(Stage::Done, 1.0);
    Ok(())
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::cell::RefCell;

    /// Records what would happen on Windows, using folders inside a temporary directory.
    pub struct FakeSystem {
        pub root: PathBuf,
        pub entry: RefCell<Option<UninstallEntry>>,
    }
    impl FakeSystem {
        pub fn new(root: &Path) -> Self {
            Self {
                root: root.to_path_buf(),
                entry: RefCell::new(None),
            }
        }
    }
    impl System for FakeSystem {
        fn shortcut_points_to(&self, link: &Path, target: &Path) -> io::Result<bool> {
            let value = fs::read_to_string(link)?;
            Ok(safe_path::same_target(Path::new(&value), target))
        }
        fn capture_registration(&self) -> io::Result<Vec<u8>> {
            Ok(serde_json::to_vec(&*self.entry.borrow())?)
        }
        fn restore_registration(&self, snapshot: &[u8]) -> io::Result<()> {
            *self.entry.borrow_mut() = serde_json::from_slice(snapshot)?;
            Ok(())
        }
        fn folders(&self) -> io::Result<Folders> {
            let folders = Folders {
                local_app_data: self.root.join("Local"),
                app_data: self.root.join("Roaming"),
                desktop: self.root.join("Desktop"),
                start_menu: self.root.join("StartMenu"),
            };
            for dir in [&folders.desktop, &folders.start_menu] {
                fs::create_dir_all(dir)?;
            }
            Ok(folders)
        }
        fn create_shortcut(&self, link: &Path, target: &Path, _: &str) -> io::Result<()> {
            fs::write(link, target.to_string_lossy().as_bytes())
        }
        fn write_uninstall_entry(&self, entry: &UninstallEntry) -> io::Result<()> {
            *self.entry.borrow_mut() = Some(entry.clone());
            Ok(())
        }
        fn remove_uninstall_entry(&self) -> io::Result<()> {
            *self.entry.borrow_mut() = None;
            Ok(())
        }
        fn existing_install(&self) -> Option<(PathBuf, Option<String>)> {
            self.entry
                .borrow()
                .as_ref()
                .map(|entry| (entry.install_dir.clone(), Some(entry.version.clone())))
        }
    }

    fn setup(root: &Path) -> (FakeSystem, PathBuf, PathBuf) {
        let system = FakeSystem::new(root);
        let setup_exe = root.join("Prism_setup.exe");
        fs::write(&setup_exe, b"setup program").unwrap();
        let dir = default_install_dir(&system.folders().unwrap());
        (system, setup_exe, dir)
    }
    fn options(dir: &Path, desktop: bool) -> InstallOptions {
        InstallOptions {
            install_dir: dir.to_path_buf(),
            desktop_shortcut: desktop,
            start_menu_shortcut: true,
        }
    }

    #[test]
    fn install_writes_files_record_shortcuts_and_registry() {
        let root = tempfile::tempdir().unwrap();
        let (system, setup_exe, dir) = setup(root.path());
        let payload = payload::pack(&[("prism.exe", b"app v1")]).unwrap();
        let mut stages = Vec::new();
        install(
            &system,
            &payload,
            "0.1.0",
            &options(&dir, true),
            &setup_exe,
            |stage, fraction| stages.push((stage, fraction)),
        )
        .unwrap();
        assert_eq!(fs::read(dir.join(APP_EXE)).unwrap(), b"app v1");
        assert_eq!(
            fs::read(dir.join(UNINSTALLER_EXE)).unwrap(),
            b"setup program"
        );
        let record = read_record(&dir);
        assert_eq!(record.version, "0.1.0");
        assert_eq!(record.files, ["prism.exe", "uninstall.exe"]);
        assert_eq!(record.shortcuts.len(), 2);
        assert!(root.path().join("Desktop/Prism.lnk").exists());
        let entry = system.entry.borrow().clone().unwrap();
        assert_eq!(
            (entry.version.as_str(), &entry.install_dir),
            ("0.1.0", &dir)
        );
        assert_eq!(stages.last(), Some(&(Stage::Done, 1.0)));
        assert!(
            stages.windows(2).all(|pair| pair[0].1 <= pair[1].1),
            "progress only grows"
        );
    }

    #[test]
    fn update_replaces_files_drops_old_ones_and_unticked_shortcuts() {
        let root = tempfile::tempdir().unwrap();
        let (system, setup_exe, dir) = setup(root.path());
        let v1 = payload::pack(&[("prism.exe", b"v1"), ("old.dll", b"old")]).unwrap();
        install(
            &system,
            &v1,
            "0.1.0",
            &options(&dir, true),
            &setup_exe,
            |_, _| {},
        )
        .unwrap();
        fs::write(dir.join("notes.txt"), b"user file").unwrap();
        let v2 = payload::pack(&[("prism.exe", b"v2")]).unwrap();
        install(
            &system,
            &v2,
            "0.2.0",
            &options(&dir, false),
            &setup_exe,
            |_, _| {},
        )
        .unwrap();
        assert_eq!(fs::read(dir.join(APP_EXE)).unwrap(), b"v2");
        assert!(
            !dir.join("old.dll").exists(),
            "file no longer shipped is removed"
        );
        assert!(
            dir.join("notes.txt").exists(),
            "user files are never touched"
        );
        assert!(!root.path().join("Desktop/Prism.lnk").exists());
        assert!(root.path().join("StartMenu/Prism.lnk").exists());
        assert_eq!(read_record(&dir).version, "0.2.0");
    }

    #[test]
    fn unsafe_install_folders_are_refused_before_writing() {
        let root = tempfile::tempdir().unwrap();
        let (system, setup_exe, _) = setup(root.path());
        let payload = payload::pack(&[("prism.exe", b"v1")]).unwrap();
        let busy = root.path().join("Documents");
        fs::create_dir_all(&busy).unwrap();
        fs::write(busy.join("thesis.docx"), b"important").unwrap();
        let error = install(
            &system,
            &payload,
            "0.1.0",
            &options(&busy, false),
            &setup_exe,
            |_, _| {},
        )
        .unwrap_err();
        assert!(error.contains("not empty"));
        assert!(!busy.join(APP_EXE).exists());
        assert!(validate_install_dir(Path::new("relative\\path")).is_err());
        assert!(system.entry.borrow().is_none());
    }

    #[test]
    fn uninstall_removes_only_recorded_items_and_optionally_data() {
        let root = tempfile::tempdir().unwrap();
        let (system, setup_exe, dir) = setup(root.path());
        let payload = payload::pack(&[("prism.exe", b"v1"), ("res/a.bin", b"a")]).unwrap();
        install(
            &system,
            &payload,
            "0.1.0",
            &options(&dir, true),
            &setup_exe,
            |_, _| {},
        )
        .unwrap();
        let folders = system.folders().unwrap();
        let data = folders.app_data.join(APP_IDENTIFIER);
        fs::create_dir_all(&data).unwrap();
        fs::write(data.join("settings.json"), b"{}").unwrap();

        // Keep data (the default): app files go, settings stay.
        uninstall(&system, &dir, false, |_, _| {}).unwrap();
        assert!(!dir.exists(), "empty install folder removed");
        assert!(!root.path().join("Desktop/Prism.lnk").exists());
        assert!(data.join("settings.json").exists());
        assert!(system.entry.borrow().is_none());

        // Reinstall with a user file in the folder, then remove everything including data.
        install(
            &system,
            &payload,
            "0.1.0",
            &options(&dir, false),
            &setup_exe,
            |_, _| {},
        )
        .unwrap();
        fs::write(dir.join("mine.txt"), b"keep me").unwrap();
        uninstall(&system, &dir, true, |_, _| {}).unwrap();
        assert!(
            dir.join("mine.txt").exists(),
            "folder kept because of the user's file"
        );
        assert!(!dir.join(APP_EXE).exists() && !dir.join("res").exists());
        assert!(!data.exists(), "data removed when asked");
    }

    #[test]
    fn required_space_includes_staging_uninstaller_and_old_file_backups_once() {
        let temporary = tempfile::tempdir().unwrap();
        let dir = temporary.path().join("app");
        assert_eq!(
            required_free_bytes(&dir, 100, 20).unwrap(),
            1024 * 1024 + 120
        );
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join(APP_EXE), b"old").unwrap();
        fs::write(dir.join(UNINSTALLER_EXE), b"setup").unwrap();
        let record = InstallRecord {
            version: "0.1.0".into(),
            files: vec![APP_EXE.into(), APP_EXE.into(), UNINSTALLER_EXE.into()],
            shortcuts: vec![],
        };
        let bytes = serde_json::to_vec(&record).unwrap();
        fs::write(dir.join(RECORD_FILE), &bytes).unwrap();
        assert_eq!(
            required_free_bytes(&dir, 100, 20).unwrap(),
            1024 * 1024 + 120 + 3 + 5 + bytes.len() as u64
        );
    }
}
