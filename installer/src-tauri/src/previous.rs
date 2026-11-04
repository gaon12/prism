//! Retain one previous app version independently of transient failure recovery.
//! Archives contain app files only. User settings, documents and indexes are never
//! copied, rolled back or deleted here. Hashes detect damage, not a malicious user
//! who can edit both the archive and its metadata under the same Windows account.
use crate::{
    install::{self, InstallRecord, System},
    journal::{self, FileDigest},
    payload, safe_path,
    transaction::Files,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

const MANIFEST: &str = "previous.json";
const PREFIX: &str = ".prism-previous-";
fn invalid() -> io::Error {
    io::Error::other("The previous-version backup is damaged or unsafe; it was preserved.")
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Stamp {
    name: String,
    digest: FileDigest,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Manifest {
    schema_version: u32,
    install_root: PathBuf,
    previous: InstallRecord,
    previous_files: Vec<Stamp>,
    current_version: String,
    current_record: FileDigest,
    current_files: Vec<Stamp>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviousVersion {
    pub version: String,
    pub current_version: String,
    pub backup_bytes: u64,
    pub required_bytes: u64,
}

fn archive_name(name: &str) -> bool {
    name.strip_prefix(PREFIX)
        .is_some_and(|suffix| journal::workspace_name(&format!(".prism-setup-{suffix}")))
}
fn checked_directory(root: &Path, directory: &Path) -> io::Result<PathBuf> {
    let root = safe_path::root(root)?;
    let directory = safe_path::root(directory)?;
    if directory.parent() != Some(root.as_path())
        || !directory
            .file_name()
            .and_then(|s| s.to_str())
            .is_some_and(archive_name)
    {
        return Err(invalid());
    }
    Ok(directory)
}
fn check_names(stamps: &[Stamp]) -> io::Result<()> {
    if stamps.len() > 10000 {
        return Err(invalid());
    }
    let mut seen = HashSet::new();
    let mut total = 0u64;
    for stamp in stamps {
        safe_path::relative_name(&stamp.name)?;
        if stamp.name.to_ascii_lowercase().starts_with(".prism-")
            || stamp.name.eq_ignore_ascii_case(install::RECORD_FILE)
            || !seen.insert(stamp.name.to_lowercase())
            || stamp.digest.length > 2 * 1024 * 1024 * 1024
        {
            return Err(invalid());
        }
        total = total.checked_add(stamp.digest.length).ok_or_else(invalid)?;
    }
    if total > 8 * 1024 * 1024 * 1024 {
        return Err(invalid());
    }
    if !stamps.iter().any(|s| s.name == install::APP_EXE)
        || !stamps.iter().any(|s| s.name == install::UNINSTALLER_EXE)
    {
        return Err(invalid());
    }
    Ok(())
}
fn same_names(names: &[String], stamps: &[Stamp]) -> bool {
    names.len() == stamps.len() && names.iter().zip(stamps).all(|(a, b)| a == &b.name)
}
fn copy(source: &Path, destination: &Path, expected: &FileDigest) -> io::Result<()> {
    safe_path::no_links(source)?;
    safe_path::no_links(destination)?;
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    let mut input = fs::File::open(source)?;
    if io::copy(&mut input, &mut output)? != expected.length {
        return Err(invalid());
    }
    output.sync_all()?;
    drop(output);
    if journal::digest(destination)? != *expected {
        return Err(invalid());
    }
    Ok(())
}

pub struct Prepared {
    root: PathBuf,
    directory: PathBuf,
    previous: InstallRecord,
    stamps: Vec<Stamp>,
}
impl Prepared {
    pub fn new(root: &Path, previous: &InstallRecord, files: &Files) -> io::Result<Self> {
        let root = safe_path::root(root)?;
        let directory = files.directory.join("previous");
        fs::create_dir(&directory)?;
        fs::create_dir(directory.join("files"))?;
        let mut stamps = Vec::new();
        for name in &previous.files {
            let original = files.original(&root, name)?;
            let digest = journal::digest(&original)?;
            let target = safe_path::create_parent(&directory.join("files"), name)?;
            copy(&original, &target, &digest)?;
            stamps.push(Stamp {
                name: name.clone(),
                digest,
            });
        }
        check_names(&stamps)?;
        Ok(Self {
            root,
            directory,
            previous: previous.clone(),
            stamps,
        })
    }
    /// Publish before the install transaction commits. A crash before that commit
    /// is recovered first, so the archive cannot authorize restoring an unfinished update.
    pub fn publish(self, current: &InstallRecord) -> io::Result<PathBuf> {
        let mut current_files = Vec::new();
        for name in &current.files {
            current_files.push(Stamp {
                name: name.clone(),
                digest: journal::digest(&safe_path::file(&self.root, name)?)?,
            });
        }
        check_names(&current_files)?;
        let manifest = Manifest {
            schema_version: 1,
            install_root: self.root.clone(),
            previous: self.previous,
            previous_files: self.stamps,
            current_version: current.version.clone(),
            current_record: journal::digest(&self.root.join(install::RECORD_FILE))?,
            current_files,
        };
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(self.directory.join(MANIFEST))?;
        let bytes = serde_json::to_vec_pretty(&manifest)?;
        if bytes.len() > 8 * 1024 * 1024 {
            return Err(invalid());
        }
        output.write_all(&bytes)?;
        output.sync_all()?;
        drop(output);
        let workspace = self
            .directory
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .ok_or_else(invalid)?;
        let suffix = workspace
            .strip_prefix(".prism-setup-")
            .ok_or_else(invalid)?;
        let destination = self.root.join(format!("{PREFIX}{suffix}"));
        safe_path::no_links(&destination)?;
        if destination.exists() {
            return Err(invalid());
        }
        journal::publish_directory(&self.directory, &destination)?;
        Ok(destination)
    }
}

struct Archive {
    root: PathBuf,
    directory: PathBuf,
    manifest: Manifest,
}
impl Archive {
    fn open(root: &Path, directory: &Path) -> io::Result<Self> {
        let root = safe_path::root(root)?;
        let directory = checked_directory(&root, directory)?;
        let manifest: Manifest = journal::read_bounded(&directory.join(MANIFEST), 8 * 1024 * 1024)?;
        if manifest.schema_version != 1
            || manifest.current_record.length > 1024 * 1024
            || manifest.install_root != root
            || manifest.previous.version.is_empty()
            || manifest.previous.version.len() > 128
            || manifest.current_version.is_empty()
            || manifest.current_version.len() > 128
            || manifest.previous.version == manifest.current_version
            || manifest.previous.shortcuts.len() > 2
        {
            return Err(invalid());
        }
        check_names(&manifest.previous_files)?;
        check_names(&manifest.current_files)?;
        if !same_names(&manifest.previous.files, &manifest.previous_files) {
            return Err(invalid());
        }
        Ok(Self {
            root,
            directory,
            manifest,
        })
    }
    fn summary(&self) -> PreviousVersion {
        PreviousVersion {
            version: self.manifest.previous.version.clone(),
            current_version: self.manifest.current_version.clone(),
            backup_bytes: self
                .manifest
                .previous_files
                .iter()
                .map(|s| s.digest.length)
                .sum(),
            required_bytes: self
                .manifest
                .previous_files
                .iter()
                .chain(&self.manifest.current_files)
                .map(|stamp| stamp.digest.length)
                .sum::<u64>()
                + self.manifest.current_record.length
                + 1024 * 1024,
        }
    }
    fn file(&self, name: &str) -> io::Result<PathBuf> {
        safe_path::file(&self.directory.join("files"), name)
    }
    fn matches_record(&self) -> io::Result<bool> {
        let record = safe_path::file(&self.root, install::RECORD_FILE)?;
        if fs::metadata(&record)?.len() > 1024 * 1024 {
            return Err(invalid());
        }
        Ok(journal::digest(&self.root.join(install::RECORD_FILE))? == self.manifest.current_record)
    }
    fn verify(&self, system: &dyn System) -> Result<(), String> {
        let bad = || {
            "The backup or current installation changed. No version files were replaced."
                .to_string()
        };
        if !self.matches_record().map_err(|_| bad())? {
            return Err(bad());
        }
        let current = install::checked_record(&self.root)?;
        if current.version != self.manifest.current_version
            || !same_names(&current.files, &self.manifest.current_files)
        {
            return Err(bad());
        }
        if !system.existing_install().is_some_and(|(dir, version)| {
            safe_path::same_target(&dir, &self.root)
                && version.as_deref() == Some(current.version.as_str())
        }) {
            return Err("Windows registration no longer belongs to this installation. Backups were preserved.".into());
        }
        for stamp in &self.manifest.current_files {
            if journal::digest(&safe_path::file(&self.root, &stamp.name).map_err(|_| bad())?)
                .map_err(|_| bad())?
                != stamp.digest
            {
                return Err(bad());
            }
        }
        for stamp in &self.manifest.previous_files {
            if journal::digest(&self.file(&stamp.name).map_err(|_| bad())?).map_err(|_| bad())?
                != stamp.digest
            {
                return Err(bad());
            }
        }
        Ok(())
    }
    /// Delete only an archive with known structure. Unexpected files/reparse points
    /// keep the entire remaining archive; they are not treated as disposable app data.
    fn cleanup(&self) -> io::Result<()> {
        checked_directory(&self.root, &self.directory)?;
        let files = self.directory.join("files");
        let mut allowed = HashSet::from([self.directory.join(MANIFEST), files.clone()]);
        for stamp in &self.manifest.previous_files {
            let path = self.file(&stamp.name)?;
            let mut parent = path.parent();
            while let Some(p) = parent.filter(|p| p.starts_with(&files) && *p != self.directory) {
                allowed.insert(p.to_path_buf());
                parent = p.parent();
            }
            allowed.insert(path);
        }
        let mut stack = vec![self.directory.clone()];
        let mut folders = Vec::new();
        let mut count = 0;
        while let Some(folder) = stack.pop() {
            for entry in fs::read_dir(&folder)? {
                let entry = entry?;
                let path = entry.path();
                safe_path::no_links(&path)?;
                count += 1;
                if count > 30000 || !allowed.contains(&path) {
                    return Err(invalid());
                }
                if entry.file_type()?.is_dir() {
                    folders.push(path.clone());
                    stack.push(path);
                }
            }
        }
        // Validate every surviving owned body before deleting the first one.
        // Missing bodies can be a prior interrupted cleanup; changed bodies are
        // preserved for inspection rather than silently treated as disposable.
        for stamp in &self.manifest.previous_files {
            let path = self.file(&stamp.name)?;
            match fs::metadata(&path) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
                Ok(_) => {
                    if journal::digest(&path)? != stamp.digest {
                        return Err(invalid());
                    }
                }
            }
        }
        for stamp in &self.manifest.previous_files {
            match fs::remove_file(self.file(&stamp.name)?) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
                _ => {}
            }
        }
        folders.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
        for folder in folders {
            fs::remove_dir(folder)?;
        }
        fs::remove_file(self.directory.join(MANIFEST))?;
        fs::remove_dir(&self.directory)
    }
}

fn archives(root: &Path) -> io::Result<Vec<Archive>> {
    let mut archives = Vec::new();
    if !root.exists() {
        return Ok(archives);
    }
    safe_path::root(root)?;
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_name().to_str().is_some_and(archive_name) {
            if archives.len() == 16 {
                return Err(invalid());
            }
            archives.push(Archive::open(root, &entry.path())?);
        }
    }
    Ok(archives)
}
fn matching(root: &Path) -> io::Result<Option<Archive>> {
    let mut found = None;
    for archive in archives(root)? {
        if archive.matches_record()? {
            if found.is_some() {
                return Err(invalid());
            }
            found = Some(archive);
        }
    }
    Ok(found)
}
/// Metadata-only: large app bodies are verified by the background restore operation.
pub fn available(root: &Path) -> Result<Option<PreviousVersion>, String> {
    matching(root)
        .map(|a| a.map(|a| a.summary()))
        .map_err(|e| e.to_string())
}
/// Called only after successful install/restore (or removal). Damaged or unknown
/// directories stay in place instead of blocking completion or selecting more paths.
pub fn prune(root: &Path, keep: Option<&Path>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        if entry.file_name().to_str().is_some_and(archive_name)
            && !keep.is_some_and(|path| safe_path::same_target(path, &entry.path()))
        {
            if let Ok(archive) = Archive::open(root, &entry.path()) {
                let _ = archive.cleanup();
            }
        }
    }
}

impl install::InstallSource for Archive {
    fn list(&self) -> io::Result<Vec<payload::PayloadFile>> {
        Ok(self
            .manifest
            .previous_files
            .iter()
            .filter(|s| s.name != install::UNINSTALLER_EXE)
            .map(|s| payload::PayloadFile {
                name: s.name.clone(),
                size: s.digest.length,
            })
            .collect())
    }
    fn stage(&self, destination: &Path, progress: &mut dyn FnMut(u64, u64)) -> io::Result<()> {
        let total: u64 = self
            .manifest
            .previous_files
            .iter()
            .filter(|s| s.name != install::UNINSTALLER_EXE)
            .map(|s| s.digest.length)
            .sum();
        let mut done = 0;
        for stamp in self
            .manifest
            .previous_files
            .iter()
            .filter(|s| s.name != install::UNINSTALLER_EXE)
        {
            let target = safe_path::create_parent(destination, &stamp.name)?;
            copy(&self.file(&stamp.name)?, &target, &stamp.digest)?;
            done += stamp.digest.length;
            progress(done, total);
        }
        Ok(())
    }
    fn setup_exe(&self) -> PathBuf {
        self.directory.join("files").join(install::UNINSTALLER_EXE)
    }
    fn verify_current(&self, system: &dyn System, root: &Path) -> Result<(), String> {
        if safe_path::root(root).map_err(|e| e.to_string())? != self.root {
            return Err(invalid().to_string());
        }
        self.verify(system)
    }
}

pub fn restore(
    system: &dyn System,
    root: &Path,
    expected_version: &str,
    mut progress: impl FnMut(install::Stage, f64),
) -> Result<(), String> {
    if expected_version.is_empty() || expected_version.len() > 128 {
        return Err("Select a valid previous version before restoring.".into());
    }
    // Recovery can change the live record. Finish it under the same folder lock
    // before selecting an archive, then recheck the source under the install lock.
    {
        let _lock = crate::folder_lock::FolderLock::acquire(root).map_err(|e| e.to_string())?;
        if crate::recovery::has_workspaces(root) {
            progress(install::Stage::Recovering, 0.0);
            crate::recovery::recover(system, root)?;
        }
    }
    let archive = matching(root)
        .map_err(|e| e.to_string())?
        .ok_or("No previous-version backup matches this installation.")?;
    if archive.manifest.previous.version != expected_version {
        return Err(
            "The previous version changed. Review the backup information and try again.".into(),
        );
    }
    let folders = system.folders().map_err(|e| e.to_string())?;
    if archive.manifest.previous.shortcuts.iter().any(|path| {
        safe_path::shortcut(path, &folders.desktop).is_err()
            && safe_path::shortcut(path, &folders.start_menu).is_err()
    }) {
        return Err(invalid().to_string());
    }
    // The install engine checks all recorded shortcut roots again under its lock.
    let options = install::InstallOptions {
        install_dir: root.into(),
        desktop_shortcut: archive
            .manifest
            .previous
            .shortcuts
            .iter()
            .any(|p| safe_path::shortcut(p, &folders.desktop).is_ok()),
        start_menu_shortcut: archive
            .manifest
            .previous
            .shortcuts
            .iter()
            .any(|p| safe_path::shortcut(p, &folders.start_menu).is_ok()),
    };
    install::install_from_source(
        system,
        &archive,
        &archive.manifest.previous.version,
        &options,
        false,
        progress,
    )?;
    prune(root, None);
    Ok(())
}
