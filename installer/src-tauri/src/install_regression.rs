//! Failure and untrusted-record tests. No real registry, shell shortcut or app is used.
use crate::{
    install::{
        self, tests::FakeSystem, Folders, InstallOptions, InstallRecord, System, UninstallEntry,
    },
    payload,
};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

struct Fault<'a> {
    inner: &'a FakeSystem,
    shortcut: bool,
    registration: bool,
    restore: bool,
}
impl System for Fault<'_> {
    fn shortcut_points_to(&self, link: &Path, target: &Path) -> io::Result<bool> {
        self.inner.shortcut_points_to(link, target)
    }
    fn folders(&self) -> io::Result<Folders> {
        self.inner.folders()
    }
    fn capture_registration(&self) -> io::Result<Vec<u8>> {
        self.inner.capture_registration()
    }
    fn restore_registration(&self, snapshot: &[u8]) -> io::Result<()> {
        if self.restore {
            return Err(io::Error::other("synthetic recovery failure"));
        }
        self.inner.restore_registration(snapshot)
    }
    fn create_shortcut(&self, link: &Path, target: &Path, description: &str) -> io::Result<()> {
        self.inner.create_shortcut(link, target, description)?;
        if self.shortcut {
            return Err(io::Error::other("synthetic shortcut failure"));
        }
        Ok(())
    }
    fn write_uninstall_entry(&self, entry: &UninstallEntry) -> io::Result<()> {
        self.inner.write_uninstall_entry(entry)?;
        if self.registration {
            return Err(io::Error::other("synthetic late registration failure"));
        }
        Ok(())
    }
    fn remove_uninstall_entry(&self) -> io::Result<()> {
        self.inner.remove_uninstall_entry()
    }
    fn existing_install(&self) -> Option<(PathBuf, Option<String>)> {
        self.inner.existing_install()
    }
}
fn options(dir: &Path) -> InstallOptions {
    InstallOptions {
        install_dir: dir.into(),
        desktop_shortcut: true,
        start_menu_shortcut: true,
    }
}
fn write_record(dir: &Path, record: &InstallRecord) {
    fs::write(
        dir.join(install::RECORD_FILE),
        serde_json::to_vec(record).unwrap(),
    )
    .unwrap();
}
fn setup(root: &Path) -> (FakeSystem, PathBuf, PathBuf) {
    let system = FakeSystem::new(root);
    let dir = root.join("app");
    let setup = root.join("setup.exe");
    fs::write(&setup, b"synthetic setup, never executed").unwrap();
    (system, dir, setup)
}
fn install_old(system: &FakeSystem, dir: &Path, setup: &Path) {
    let bytes =
        payload::pack(&[("prism.exe", b"old app"), ("res/old.bin", b"old resource")]).unwrap();
    install::install(system, &bytes, "0.1.0", &options(dir), setup, |_, _| {}).unwrap();
}

#[test]
fn unsafe_record_entries_are_rejected_before_any_application_file_is_removed() {
    for name in [
        "../personal.txt".to_string(),
        std::env::temp_dir()
            .join("personal.txt")
            .to_string_lossy()
            .into_owned(),
        "prism.exe:stream".into(),
        ".prism-previous-1-0/files/prism.exe".into(),
        ".prism-setup-1-0/state.json".into(),
        "install.json".into(),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let (system, dir, setup) = setup(temp.path());
        install_old(&system, &dir, &setup);
        let outside = temp.path().join("personal.txt");
        fs::write(&outside, b"keep").unwrap();
        let mut record = install::read_record(&dir);
        record.files.push(name);
        write_record(&dir, &record);
        assert!(install::uninstall(&system, &dir, false, |_, _| {}).is_err());
        assert_eq!(fs::read(dir.join("prism.exe")).unwrap(), b"old app");
        assert_eq!(fs::read(&outside).unwrap(), b"keep");
    }
}

#[test]
fn arbitrary_recorded_shortcuts_are_not_deleted() {
    let temp = tempfile::tempdir().unwrap();
    let (system, dir, setup) = setup(temp.path());
    install_old(&system, &dir, &setup);
    let outside = temp.path().join("personal.txt");
    fs::write(&outside, b"keep").unwrap();
    let mut record = install::read_record(&dir);
    record.shortcuts = vec![outside.clone()];
    write_record(&dir, &record);
    assert!(install::uninstall(&system, &dir, false, |_, _| {}).is_err());
    assert_eq!(fs::read(&outside).unwrap(), b"keep");
    assert!(dir.join("prism.exe").exists());
}

#[test]
fn late_failures_restore_app_record_resources_shortcuts_and_registration() {
    for shortcut_failure in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let (system, dir, setup) = setup(temp.path());
        install_old(&system, &dir, &setup);
        fs::write(dir.join("personal.txt"), b"user content").unwrap();
        let record = fs::read(dir.join(install::RECORD_FILE)).unwrap();
        let entry = system.entry.borrow().clone();
        let folders = system.folders().unwrap();
        let old_link = fs::read(folders.desktop.join("Prism.lnk")).unwrap();
        let fault = Fault {
            inner: &system,
            shortcut: shortcut_failure,
            registration: !shortcut_failure,
            restore: false,
        };
        let bytes =
            payload::pack(&[("prism.exe", b"new app"), ("res/new.bin", b"new resource")]).unwrap();
        let error = install::install(&fault, &bytes, "0.2.0", &options(&dir), &setup, |_, _| {})
            .unwrap_err();
        assert!(
            error.contains("previous installation was restored"),
            "{error}"
        );
        assert_eq!(fs::read(dir.join("prism.exe")).unwrap(), b"old app");
        assert_eq!(fs::read(dir.join("res/old.bin")).unwrap(), b"old resource");
        assert!(!dir.join("res/new.bin").exists());
        assert_eq!(fs::read(dir.join("personal.txt")).unwrap(), b"user content");
        assert_eq!(fs::read(dir.join(install::RECORD_FILE)).unwrap(), record);
        assert_eq!(
            fs::read(folders.desktop.join("Prism.lnk")).unwrap(),
            old_link
        );
        assert_eq!(*system.entry.borrow(), entry);
        assert!(!fs::read_dir(&dir).unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".prism-setup-")));
    }
}

#[test]
fn failed_first_install_removes_new_files_and_registration() {
    let temp = tempfile::tempdir().unwrap();
    let (system, dir, setup) = setup(temp.path());
    let fault = Fault {
        inner: &system,
        shortcut: false,
        registration: true,
        restore: false,
    };
    let bytes = payload::pack(&[("prism.exe", b"new")]).unwrap();
    assert!(install::install(&fault, &bytes, "0.1.0", &options(&dir), &setup, |_, _| {}).is_err());
    assert!(!dir.join("prism.exe").exists());
    assert!(!dir.join("install.json").exists());
    assert!(!system.folders().unwrap().desktop.join("Prism.lnk").exists());
    assert!(system.entry.borrow().is_none());
}

#[test]
fn incomplete_recovery_is_reported_and_backups_are_kept() {
    let temp = tempfile::tempdir().unwrap();
    let (system, dir, setup) = setup(temp.path());
    install_old(&system, &dir, &setup);
    let fault = Fault {
        inner: &system,
        shortcut: false,
        registration: true,
        restore: true,
    };
    let bytes = payload::pack(&[("prism.exe", b"new")]).unwrap();
    let error =
        install::install(&fault, &bytes, "0.2.0", &options(&dir), &setup, |_, _| {}).unwrap_err();
    assert!(error.contains("Recovery was incomplete"));
    assert!(fs::read_dir(&dir).unwrap().any(|entry| entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".prism-setup-")));
    crate::recovery::recover(&system, &dir).unwrap();
    assert_eq!(fs::read(dir.join("prism.exe")).unwrap(), b"old app");
    assert_eq!(system.entry.borrow().as_ref().unwrap().version, "0.1.0");
    assert!(!crate::recovery::has_workspaces(&dir));
}

#[test]
fn damaged_records_and_colliding_user_files_block_updates_without_mutation() {
    let temp = tempfile::tempdir().unwrap();
    let (system, dir, setup) = setup(temp.path());
    install_old(&system, &dir, &setup);
    let record = fs::read(dir.join("install.json")).unwrap();
    fs::write(dir.join("install.json"), b"broken record").unwrap();
    let bytes = payload::pack(&[("prism.exe", b"new")]).unwrap();
    assert!(install::install(&system, &bytes, "0.2.0", &options(&dir), &setup, |_, _| {}).is_err());
    assert_eq!(fs::read(dir.join("prism.exe")).unwrap(), b"old app");
    assert_eq!(
        fs::read(dir.join("install.json")).unwrap(),
        b"broken record"
    );
    fs::write(dir.join("install.json"), record).unwrap();
    fs::write(dir.join("user.bin"), b"personal").unwrap();
    let bytes = payload::pack(&[("prism.exe", b"new"), ("user.bin", b"release")]).unwrap();
    assert!(install::install(&system, &bytes, "0.2.0", &options(&dir), &setup, |_, _| {}).is_err());
    assert_eq!(fs::read(dir.join("user.bin")).unwrap(), b"personal");
}

#[test]
fn user_modified_shortcut_is_preserved_before_uninstall_changes_any_files() {
    let temp = tempfile::tempdir().unwrap();
    let (system, dir, setup) = setup(temp.path());
    install_old(&system, &dir, &setup);
    let link = system.folders().unwrap().desktop.join("Prism.lnk");
    fs::write(
        &link,
        temp.path().join("other.exe").to_string_lossy().as_bytes(),
    )
    .unwrap();
    let before = fs::read(&link).unwrap();
    assert!(install::uninstall(&system, &dir, false, |_, _| {}).is_err());
    assert_eq!(fs::read(&link).unwrap(), before);
    assert_eq!(fs::read(dir.join("prism.exe")).unwrap(), b"old app");
}

#[test]
fn junctions_cannot_redirect_payload_writes_or_recorded_file_deletions() {
    let temp = tempfile::tempdir().unwrap();
    let outside = temp.path().join("personal");
    let dir = temp.path().join("app");
    fs::create_dir(&outside).unwrap();
    fs::create_dir(&dir).unwrap();
    fs::write(outside.join("note.txt"), b"keep").unwrap();
    let link = dir.join("resources");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Values are passed as environment data, never interpolated into shell code.
        let output = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", "New-Item -ItemType Junction -Path $env:PRISM_TEST_LINK -Target $env:PRISM_TEST_TARGET -ErrorAction Stop | Out-Null"])
            .env("PRISM_TEST_LINK", &link).env("PRISM_TEST_TARGET", &outside)
            .creation_flags(0x0800_0000).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    #[cfg(not(windows))]
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    let bytes = payload::pack(&[("resources/note.txt", b"overwrite")]).unwrap();
    assert!(payload::extract(&bytes, &dir, |_, _| {}).is_err());
    write_record(
        &dir,
        &InstallRecord {
            version: "0.1.0".into(),
            files: vec!["resources/note.txt".into()],
            shortcuts: vec![],
        },
    );
    let system = FakeSystem::new(temp.path());
    assert!(install::uninstall(&system, &dir, false, |_, _| {}).is_err());
    assert_eq!(fs::read(outside.join("note.txt")).unwrap(), b"keep");
    assert!(install::validate_install_dir(&link.join("new-install")).is_err());
}

/// The component notices ship as ordinary recorded payload files under `notices/`:
/// installed byte for byte, replaced or dropped on update like any app file, removed on
/// uninstall, while a user's own file in the same folder is never touched.
#[test]
fn component_notices_are_installed_exactly_and_only_recorded_ones_are_removed() {
    let temp = tempfile::tempdir().unwrap();
    let (system, dir, setup) = setup(temp.path());
    let notices: [(&str, &[u8]); 4] = [
        (
            "notices/pdf-renderer.txt",
            b"pdf renderer notice\r\n\xEF\xBB\xBF",
        ),
        ("notices/document-pdf.txt", b"document pdf notice"),
        ("notices/document-hwpx.txt", b"document hwpx notice"),
        ("notices/office-passwords.txt", b"office passwords notice"),
    ];
    let mut files: Vec<(&str, &[u8])> = vec![("prism.exe", b"app v1")];
    files.extend(notices);
    let bytes = payload::pack(&files).unwrap();
    install::install(&system, &bytes, "0.1.0", &options(&dir), &setup, |_, _| {}).unwrap();
    for (name, content) in notices {
        assert_eq!(fs::read(dir.join(name)).unwrap(), content, "{name}");
        assert!(install::read_record(&dir)
            .files
            .iter()
            .any(|file| file == name));
    }

    // A user's own file next to the notices is not ours.
    fs::write(dir.join("notices/my-note.txt"), b"user note").unwrap();

    // An update that no longer ships one notice removes exactly that recorded file.
    let mut updated: Vec<(&str, &[u8])> = vec![("prism.exe", b"app v2")];
    updated.extend(
        notices
            .iter()
            .copied()
            .filter(|(name, _)| !name.contains("hwpx")),
    );
    let bytes = payload::pack(&updated).unwrap();
    install::install(&system, &bytes, "0.2.0", &options(&dir), &setup, |_, _| {}).unwrap();
    assert!(!dir.join("notices/document-hwpx.txt").exists());
    assert_eq!(
        fs::read(dir.join("notices/pdf-renderer.txt")).unwrap(),
        notices[0].1
    );
    assert_eq!(
        fs::read(dir.join("notices/my-note.txt")).unwrap(),
        b"user note"
    );

    // Uninstall removes the recorded notices and keeps the user's file.
    install::uninstall(&system, &dir, false, |_, _| {}).unwrap();
    for (name, _) in notices {
        assert!(!dir.join(name).exists(), "{name}");
    }
    assert_eq!(
        fs::read(dir.join("notices/my-note.txt")).unwrap(),
        b"user note"
    );
}
