//! Real archive and install engine; all app/setup bytes and OS effects are inert.
use crate::{
    install::{self, tests::FakeSystem, Folders, InstallOptions, System, UninstallEntry},
    payload, previous,
};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

fn options(dir: &Path, desktop: bool) -> InstallOptions {
    InstallOptions {
        install_dir: dir.into(),
        desktop_shortcut: desktop,
        start_menu_shortcut: true,
    }
}
fn install_version(
    system: &dyn System,
    dir: &Path,
    setup: &Path,
    version: &str,
    app: &[u8],
    resource: &str,
    desktop: bool,
) {
    let bytes = payload::pack(&[("prism.exe", app), (resource, b"resource")]).unwrap();
    install::install(
        system,
        &bytes,
        version,
        &options(dir, desktop),
        setup,
        |_, _| {},
    )
    .unwrap();
}
fn base(root: &Path) -> (FakeSystem, PathBuf, PathBuf) {
    let system = FakeSystem::new(root);
    let dir = root.join("app");
    let setup = root.join("setup.exe");
    fs::write(&setup, b"old setup").unwrap();
    install_version(
        &system,
        &dir,
        &setup,
        "0.1.0",
        b"old app",
        "res/old.bin",
        true,
    );
    assert!(previous::available(&dir).unwrap().is_none());
    fs::write(dir.join("personal.txt"), b"user file").unwrap();
    fs::write(&setup, b"new setup").unwrap();
    install_version(
        &system,
        &dir,
        &setup,
        "0.2.0",
        b"new app",
        "res/new.bin",
        false,
    );
    (system, dir, setup)
}
fn archive(dir: &Path) -> PathBuf {
    fs::read_dir(dir)
        .unwrap()
        .map(Result::unwrap)
        .find(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with(".prism-previous-")
        })
        .unwrap()
        .path()
}

#[test]
fn a_previous_version_restores_files_uninstaller_links_and_registration_only() {
    let temp = tempfile::tempdir().unwrap();
    let (system, dir, _) = base(temp.path());
    let folders = system.folders().unwrap();
    let data = folders.app_data.join(install::APP_IDENTIFIER);
    fs::create_dir_all(&data).unwrap();
    fs::write(data.join("settings.json"), b"newer settings").unwrap();
    let summary = previous::available(&dir).unwrap().unwrap();
    assert_eq!(summary.version, "0.1.0");
    assert_eq!(summary.current_version, "0.2.0");
    assert!(summary.required_bytes > summary.backup_bytes);
    previous::restore(&system, &dir, "0.1.0", |_, _| {}).unwrap();
    assert_eq!(fs::read(dir.join("prism.exe")).unwrap(), b"old app");
    assert_eq!(fs::read(dir.join("uninstall.exe")).unwrap(), b"old setup");
    assert!(dir.join("res/old.bin").exists());
    assert!(!dir.join("res/new.bin").exists());
    assert_eq!(install::read_record(&dir).version, "0.1.0");
    assert_eq!(
        system.existing_install().unwrap().1.as_deref(),
        Some("0.1.0")
    );
    assert!(folders.desktop.join("Prism.lnk").exists());
    assert_eq!(fs::read(dir.join("personal.txt")).unwrap(), b"user file");
    assert_eq!(
        fs::read(data.join("settings.json")).unwrap(),
        b"newer settings"
    );
    assert!(previous::available(&dir).unwrap().is_none());
    assert!(previous::restore(&system, &dir, "0.1.0", |_, _| {}).is_err());
}

#[test]
fn successful_update_keeps_only_its_immediate_predecessor_and_binds_selection() {
    let temp = tempfile::tempdir().unwrap();
    let (system, dir, setup) = base(temp.path());
    install_version(
        &system,
        &dir,
        &setup,
        "0.3.0",
        b"third app",
        "res/third.bin",
        false,
    );
    assert_eq!(previous::available(&dir).unwrap().unwrap().version, "0.2.0");
    assert_eq!(
        fs::read_dir(&dir)
            .unwrap()
            .map(Result::unwrap)
            .filter(|e| e
                .file_name()
                .to_string_lossy()
                .starts_with(".prism-previous-"))
            .count(),
        1
    );
    assert!(previous::restore(&system, &dir, "0.1.0", |_, _| {}).is_err());
    assert_eq!(fs::read(dir.join("prism.exe")).unwrap(), b"third app");
    previous::restore(&system, &dir, "0.2.0", |_, _| {}).unwrap();
    assert_eq!(fs::read(dir.join("prism.exe")).unwrap(), b"new app");
}

#[test]
fn damaged_backup_changed_current_files_and_new_collisions_never_overwrite_user_bytes() {
    for fault in 0..3 {
        let temp = tempfile::tempdir().unwrap();
        let (system, dir, _) = base(temp.path());
        let target = match fault {
            0 => archive(&dir).join("files/prism.exe"),
            1 => dir.join("prism.exe"),
            _ => dir.join("res/old.bin"),
        };
        fs::write(&target, b"changed by user").unwrap();
        assert!(previous::restore(&system, &dir, "0.1.0", |_, _| {}).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"changed by user");
        assert_eq!(install::read_record(&dir).version, "0.2.0");
        assert_eq!(
            system.existing_install().unwrap().1.as_deref(),
            Some("0.2.0")
        );
    }
}

#[test]
fn unsafe_archive_roots_names_and_schema_fail_before_writes() {
    for fault in 0..4 {
        let temp = tempfile::tempdir().unwrap();
        let (system, dir, _) = base(temp.path());
        let manifest = archive(&dir).join("previous.json");
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
        match fault {
            0 => value["installRoot"] = serde_json::json!(temp.path()),
            1 => value["previousFiles"][0]["name"] = serde_json::json!("../personal.txt"),
            2 => value["schemaVersion"] = serde_json::json!(2),
            _ => value["currentRecord"]["length"] = serde_json::json!(u64::MAX),
        }
        fs::write(manifest, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(previous::available(&dir).is_err());
        assert!(previous::restore(&system, &dir, "0.1.0", |_, _| {}).is_err());
        assert_eq!(fs::read(dir.join("prism.exe")).unwrap(), b"new app");
        assert_eq!(fs::read(dir.join("personal.txt")).unwrap(), b"user file");
    }
}

struct Fail<'a>(&'a FakeSystem);
impl System for Fail<'_> {
    fn folders(&self) -> io::Result<Folders> {
        self.0.folders()
    }
    fn capture_registration(&self) -> io::Result<Vec<u8>> {
        self.0.capture_registration()
    }
    fn restore_registration(&self, v: &[u8]) -> io::Result<()> {
        self.0.restore_registration(v)
    }
    fn create_shortcut(&self, l: &Path, t: &Path, d: &str) -> io::Result<()> {
        self.0.create_shortcut(l, t, d)
    }
    fn shortcut_points_to(&self, l: &Path, t: &Path) -> io::Result<bool> {
        self.0.shortcut_points_to(l, t)
    }
    fn write_uninstall_entry(&self, e: &UninstallEntry) -> io::Result<()> {
        self.0.write_uninstall_entry(e)?;
        Err(io::Error::other("synthetic failure after registration"))
    }
    fn remove_uninstall_entry(&self) -> io::Result<()> {
        self.0.remove_uninstall_entry()
    }
    fn existing_install(&self) -> Option<(PathBuf, Option<String>)> {
        self.0.existing_install()
    }
}
#[test]
fn failed_restore_returns_to_the_current_version_and_can_be_retried() {
    let temp = tempfile::tempdir().unwrap();
    let (system, dir, _) = base(temp.path());
    assert!(previous::restore(&Fail(&system), &dir, "0.1.0", |_, _| {}).is_err());
    assert_eq!(fs::read(dir.join("prism.exe")).unwrap(), b"new app");
    assert_eq!(fs::read(dir.join("uninstall.exe")).unwrap(), b"new setup");
    assert_eq!(
        system.existing_install().unwrap().1.as_deref(),
        Some("0.2.0")
    );
    assert_eq!(previous::available(&dir).unwrap().unwrap().version, "0.1.0");
    previous::restore(&system, &dir, "0.1.0", |_, _| {}).unwrap();
}

#[test]
fn removal_cleans_owned_backups_but_preserves_unexpected_archive_files() {
    for fault in 0..3 {
        let temp = tempfile::tempdir().unwrap();
        let (system, dir, _) = base(temp.path());
        let backup = archive(&dir);
        if fault == 1 {
            fs::write(backup.join("personal.txt"), b"not owned").unwrap();
        }
        if fault == 2 {
            fs::write(backup.join("files/prism.exe"), b"changed backup bytes").unwrap();
        }
        install::uninstall(&system, &dir, false, |_, _| {}).unwrap();
        if fault == 1 {
            assert_eq!(fs::read(backup.join("personal.txt")).unwrap(), b"not owned");
        } else if fault == 2 {
            assert_eq!(
                fs::read(backup.join("files/prism.exe")).unwrap(),
                b"changed backup bytes"
            );
            assert!(backup.join("previous.json").exists());
            assert!(
                backup.join("files/uninstall.exe").exists(),
                "validate all bodies before any deletion"
            );
        } else {
            assert!(!backup.exists());
        }
        assert_eq!(fs::read(dir.join("personal.txt")).unwrap(), b"user file");
    }
}

#[test]
fn restore_preserves_other_installation_registration_and_changed_shortcuts() {
    for registration in [true, false] {
        let temp = tempfile::tempdir().unwrap();
        let (system, dir, _) = base(temp.path());
        let link = system.folders().unwrap().desktop.join("Prism.lnk");
        if registration {
            system
                .write_uninstall_entry(&UninstallEntry {
                    version: "0.2.0".into(),
                    install_dir: temp.path().join("other-app"),
                    size_kib: 1,
                })
                .unwrap();
        } else {
            fs::write(&link, b"user shortcut to another app").unwrap();
        }
        let entry = system.capture_registration().unwrap();
        assert!(previous::restore(&system, &dir, "0.1.0", |_, _| {}).is_err());
        assert_eq!(fs::read(dir.join("prism.exe")).unwrap(), b"new app");
        assert_eq!(system.capture_registration().unwrap(), entry);
        if !registration {
            assert_eq!(fs::read(link).unwrap(), b"user shortcut to another app");
        }
    }
}

#[test]
fn identical_same_version_reinstall_keeps_the_valid_previous_archive() {
    let temp = tempfile::tempdir().unwrap();
    let (system, dir, setup) = base(temp.path());
    let retained = archive(&dir);
    install_version(
        &system,
        &dir,
        &setup,
        "0.2.0",
        b"new app",
        "res/new.bin",
        false,
    );
    assert_eq!(archive(&dir), retained);
    previous::restore(&system, &dir, "0.1.0", |_, _| {}).unwrap();
    assert_eq!(fs::read(dir.join("prism.exe")).unwrap(), b"old app");
}
