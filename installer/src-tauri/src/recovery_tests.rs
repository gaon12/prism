//! Recovery uses real transaction files and inert payload bytes, never a user's application.
use crate::{
    install::{self, tests::FakeSystem, InstallOptions, System, UninstallEntry},
    payload, recovery,
    transaction::Files,
};
use std::{
    fs,
    path::{Path, PathBuf},
};

fn base(root: &Path) -> (FakeSystem, PathBuf, PathBuf) {
    let system = FakeSystem::new(root);
    let app = root.join("app");
    let setup = root.join("setup.exe");
    fs::write(&setup, b"inert setup").unwrap();
    let bytes = payload::pack(&[("prism.exe", b"old"), ("res/old.txt", b"old resource")]).unwrap();
    install::install(&system, &bytes, "0.1.0", &options(&app), &setup, |_, _| {}).unwrap();
    fs::write(app.join("personal.txt"), b"user file").unwrap();
    (system, app, setup)
}
fn options(app: &Path) -> InstallOptions {
    InstallOptions {
        install_dir: app.into(),
        desktop_shortcut: true,
        start_menu_shortcut: true,
    }
}
fn interrupted(system: &FakeSystem, app: &Path) -> Files {
    let mut files = Files::new(app).unwrap();
    for name in ["prism.exe", "res/old.txt", "res/new.txt", "install.json"] {
        files.snapshot(app, name).unwrap();
    }
    let folders = system.folders().unwrap();
    for folder in [&folders.desktop, &folders.start_menu] {
        files.snapshot(folder, "Prism.lnk").unwrap();
    }
    fs::write(
        files.directory.join("registration.json"),
        system.capture_registration().unwrap(),
    )
    .unwrap();
    files.begin_changes().unwrap();
    fs::write(app.join("prism.exe"), b"new").unwrap();
    fs::remove_file(app.join("res/old.txt")).unwrap();
    fs::write(app.join("res/new.txt"), b"new resource").unwrap();
    system
        .write_uninstall_entry(&UninstallEntry {
            version: "0.2.0".into(),
            install_dir: app.into(),
            size_kib: 1,
        })
        .unwrap();
    files
}
fn pending(system: &FakeSystem, app: &Path) -> PathBuf {
    let files = interrupted(system, app);
    let directory = files.directory.clone();
    std::mem::forget(files);
    directory
}

#[test]
fn pending_transaction_restores_all_recorded_state_and_keeps_user_files() {
    let temp = tempfile::tempdir().unwrap();
    let (system, app, _) = base(temp.path());
    let record = fs::read(app.join("install.json")).unwrap();
    let registration = system.capture_registration().unwrap();
    let directory = pending(&system, &app);
    assert_eq!(recovery::recover(&system, &app).unwrap(), 1);
    assert_eq!(fs::read(app.join("prism.exe")).unwrap(), b"old");
    assert_eq!(fs::read(app.join("res/old.txt")).unwrap(), b"old resource");
    assert!(!app.join("res/new.txt").exists());
    assert_eq!(fs::read(app.join("install.json")).unwrap(), record);
    assert_eq!(system.capture_registration().unwrap(), registration);
    assert_eq!(fs::read(app.join("personal.txt")).unwrap(), b"user file");
    assert!(!directory.exists());
    assert_eq!(recovery::recover(&system, &app).unwrap(), 0);
}

#[test]
fn completed_installation_cleanup_never_rolls_back_even_with_missing_backups() {
    let temp = tempfile::tempdir().unwrap();
    let (system, app, _) = base(temp.path());
    let mut files = interrupted(&system, &app);
    files.finished().unwrap();
    let directory = files.directory.clone();
    std::mem::forget(files);
    // Model a process exit partway through committed-workspace cleanup.
    fs::remove_file(directory.join("backup/0")).unwrap();
    fs::remove_file(directory.join("files.json")).unwrap();
    assert_eq!(recovery::recover(&system, &app).unwrap(), 0);
    assert_eq!(fs::read(app.join("prism.exe")).unwrap(), b"new");
    assert_eq!(system.entry.borrow().as_ref().unwrap().version, "0.2.0");
    assert!(!directory.exists());
}

#[test]
fn malformed_or_escaped_journals_and_corrupt_backups_fail_before_any_recovery_write() {
    for case in [
        "backup",
        "registration",
        "root",
        "name",
        "backupPath",
        "duplicate",
        "phase",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let (system, app, _) = base(temp.path());
        let directory = pending(&system, &app);
        let registration = system.capture_registration().unwrap();
        let map = directory.join("files.json");
        let mut entries: serde_json::Value =
            serde_json::from_slice(&fs::read(&map).unwrap()).unwrap();
        match case {
            "backup" => fs::write(directory.join("backup/0"), b"damaged").unwrap(),
            "registration" => fs::write(directory.join("registration.json"), b"damaged").unwrap(),
            "root" => entries[0]["root"] = serde_json::json!(temp.path()),
            "name" => entries[0]["name"] = "../personal.txt".into(),
            "backupPath" => entries[0]["backup"] = serde_json::json!(temp.path().join("setup.exe")),
            "duplicate" => {
                let first = entries[0].clone();
                entries.as_array_mut().unwrap().push(first);
            }
            "phase" => {
                let state = directory.join("state.json");
                let mut value: serde_json::Value =
                    serde_json::from_slice(&fs::read(&state).unwrap()).unwrap();
                value["schemaVersion"] = 999.into();
                fs::write(state, serde_json::to_vec(&value).unwrap()).unwrap();
            }
            _ => unreachable!(),
        }
        if matches!(case, "root" | "name" | "backupPath" | "duplicate") {
            fs::write(&map, serde_json::to_vec(&entries).unwrap()).unwrap();
            // Recompute the checksum to exercise destination constraints independently of
            // accidental-corruption detection. The journal is not a signature authority.
            let mut state = crate::journal::Journal::open(&app, &directory).unwrap();
            state.files = Some(crate::journal::digest(&map).unwrap());
            state.save(&directory).unwrap();
        }
        assert!(recovery::recover(&system, &app).is_err(), "{case}");
        assert_eq!(fs::read(app.join("prism.exe")).unwrap(), b"new", "{case}");
        assert_eq!(fs::read(app.join("personal.txt")).unwrap(), b"user file");
        assert_eq!(system.capture_registration().unwrap(), registration);
        assert!(directory.exists());
    }
}

#[test]
fn interrupted_restore_retries_its_own_partial_copy_and_preserves_other_files() {
    let temp = tempfile::tempdir().unwrap();
    let (system, app, _) = base(temp.path());
    let directory = pending(&system, &app);
    let token = directory.file_name().unwrap().to_str().unwrap();
    let partial = app.join(format!("prism.exe.prism-recover-{token}"));
    fs::write(&partial, b"partial copy").unwrap();
    fs::write(app.join("different-recovery-file"), b"keep").unwrap();
    recovery::recover(&system, &app).unwrap();
    assert!(!partial.exists());
    assert_eq!(fs::read(app.join("prism.exe")).unwrap(), b"old");
    assert_eq!(
        fs::read(app.join("different-recovery-file")).unwrap(),
        b"keep"
    );
}

#[test]
fn preparing_work_is_discarded_but_legacy_unversioned_recovery_is_preserved() {
    let temp = tempfile::tempdir().unwrap();
    let (system, app, _) = base(temp.path());
    let files = Files::new(&app).unwrap();
    fs::write(
        files.stage().join("prism.exe"),
        b"staged but never installed",
    )
    .unwrap();
    let directory = files.directory.clone();
    std::mem::forget(files);
    assert_eq!(recovery::recover(&system, &app).unwrap(), 0);
    assert_eq!(fs::read(app.join("prism.exe")).unwrap(), b"old");
    assert!(!directory.exists());
    let legacy = app.join(".prism-setup-123-456");
    fs::create_dir(&legacy).unwrap();
    fs::write(legacy.join("files.json"), b"[]").unwrap();
    assert!(recovery::recover(&system, &app).is_err());
    assert!(legacy.join("files.json").exists());
}

#[test]
fn ambiguous_pending_transactions_are_not_replayed_in_directory_order() {
    let temp = tempfile::tempdir().unwrap();
    let (system, app, _) = base(temp.path());
    let first = pending(&system, &app);
    // Create a second complete recovery map without mutating the first backup.
    let mut second = Files::new(&app).unwrap();
    second.snapshot(&app, "prism.exe").unwrap();
    fs::write(
        second.directory.join("registration.json"),
        system.capture_registration().unwrap(),
    )
    .unwrap();
    second.begin_changes().unwrap();
    let other = second.directory.clone();
    std::mem::forget(second);
    assert!(recovery::recover(&system, &app)
        .unwrap_err()
        .contains("Multiple"));
    assert_eq!(fs::read(app.join("prism.exe")).unwrap(), b"new");
    assert!(first.exists() && other.exists());
}

#[test]
fn a_shortcut_changed_by_the_user_blocks_recovery_before_other_files_change() {
    let temp = tempfile::tempdir().unwrap();
    let (system, app, _) = base(temp.path());
    let directory = pending(&system, &app);
    let link = system.folders().unwrap().desktop.join("Prism.lnk");
    let other = temp.path().join("another-app.exe");
    fs::write(&link, other.to_string_lossy().as_bytes()).unwrap();
    let changed = fs::read(&link).unwrap();
    assert!(recovery::recover(&system, &app)
        .unwrap_err()
        .contains("shortcut changed"));
    assert_eq!(fs::read(&link).unwrap(), changed);
    assert_eq!(fs::read(app.join("prism.exe")).unwrap(), b"new");
    assert!(directory.exists());
}

#[test]
fn a_newer_registration_for_another_folder_is_not_replaced_by_old_recovery() {
    let temp = tempfile::tempdir().unwrap();
    let (system, app, _) = base(temp.path());
    let directory = pending(&system, &app);
    let another = temp.path().join("other installation");
    fs::create_dir(&another).unwrap();
    system
        .write_uninstall_entry(&UninstallEntry {
            install_dir: another,
            version: "0.9.0".into(),
            size_kib: 1,
        })
        .unwrap();
    let registration = system.capture_registration().unwrap();
    assert!(recovery::recover(&system, &app)
        .unwrap_err()
        .contains("another installation"));
    assert_eq!(system.capture_registration().unwrap(), registration);
    assert_eq!(fs::read(app.join("prism.exe")).unwrap(), b"new");
    assert!(directory.exists());
}

#[test]
fn a_shortcut_changed_during_install_is_not_overwritten_by_the_failure_rollback() {
    let temp = tempfile::tempdir().unwrap();
    let (system, app, setup) = base(temp.path());
    let link = system.folders().unwrap().desktop.join("Prism.lnk");
    let other = temp.path().join("another-app.exe");
    let bytes = payload::pack(&[("prism.exe", b"new")]).unwrap();
    let error = install::install(
        &system,
        &bytes,
        "0.2.0",
        &options(&app),
        &setup,
        |stage, _| {
            if stage == install::Stage::Shortcuts {
                fs::write(&link, other.to_string_lossy().as_bytes()).unwrap();
            }
        },
    )
    .unwrap_err();
    assert!(error.contains("recovery backups were preserved"));
    assert_eq!(fs::read(&link).unwrap(), other.to_string_lossy().as_bytes());
    assert!(recovery::has_workspaces(&app));
    assert!(recovery::recover(&system, &app).is_err());
    // The fixture opts to remove its changed link. Recovery can now finish without clobbering it.
    fs::remove_file(link).unwrap();
    recovery::recover(&system, &app).unwrap();
    assert_eq!(fs::read(app.join("prism.exe")).unwrap(), b"old");
}

#[test]
fn install_and_uninstall_recover_before_reading_the_previous_record() {
    for uninstall in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let (system, app, setup) = base(temp.path());
        pending(&system, &app);
        // The old record is intact in the backup; an interrupted write must not block recovery.
        fs::write(app.join("install.json"), b"incomplete current record").unwrap();
        let before_estimate = fs::read(app.join("prism.exe")).unwrap();
        assert!(install::required_free_bytes(&app, 100, 200).is_ok());
        assert_eq!(
            fs::read(app.join("prism.exe")).unwrap(),
            before_estimate,
            "estimate is read-only"
        );
        let mut stages = Vec::new();
        if uninstall {
            install::uninstall(&system, &app, false, |stage, _| stages.push(stage)).unwrap();
            assert!(!app.join("prism.exe").exists());
            assert!(system.entry.borrow().is_none());
        } else {
            let bytes = payload::pack(&[("prism.exe", b"third version")]).unwrap();
            install::install(
                &system,
                &bytes,
                "0.3.0",
                &options(&app),
                &setup,
                |stage, _| stages.push(stage),
            )
            .unwrap();
            assert_eq!(fs::read(app.join("prism.exe")).unwrap(), b"third version");
            assert_eq!(system.entry.borrow().as_ref().unwrap().version, "0.3.0");
        }
        assert!(stages.contains(&install::Stage::Recovering));
        assert_eq!(fs::read(app.join("personal.txt")).unwrap(), b"user file");
        assert!(!recovery::has_workspaces(&app));
    }
}

#[test]
#[cfg(windows)]
#[ignore = "Private subprocess fixture; invoked only with a validated OS-temp sandbox"]
fn crash_fixture_process() {
    use crate::install::{Folders, Stage};
    use std::io;
    let root = PathBuf::from(
        std::env::var_os("PRISM_RECOVERY_TEST_ROOT").expect("private sandbox required"),
    );
    let root = fs::canonicalize(root).unwrap();
    assert_eq!(
        root.parent().unwrap(),
        fs::canonicalize(std::env::temp_dir()).unwrap()
    );
    assert!(root
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("prism-recovery-test-"));
    assert_eq!(
        fs::read(root.join("fixture-owner.txt")).unwrap(),
        b"Prism isolated recovery fixture"
    );
    let phase = std::env::var("PRISM_RECOVERY_TEST_PHASE").unwrap();
    let system = crate::system::SandboxSystem { root: root.clone() };
    let setup = root.join("fixture-setup.exe");
    fs::write(&setup, b"inert setup; never executed").unwrap();
    let app = root.join("app");
    let old = payload::pack(&[("prism.exe", b"old"), ("res/old.txt", b"old resource")]).unwrap();
    install::install(&system, &old, "0.1.0", &options(&app), &setup, |_, _| {}).unwrap();
    fs::write(app.join("personal.txt"), b"user file").unwrap();
    // The wrapper stops after the real sandbox registration adapter has written its file,
    // but before the installer can record completion or run its ordinary rollback path.
    struct CrashAfterRegistration<'a> {
        inner: &'a dyn System,
        crash: bool,
    }
    impl System for CrashAfterRegistration<'_> {
        fn folders(&self) -> io::Result<Folders> {
            self.inner.folders()
        }
        fn capture_registration(&self) -> io::Result<Vec<u8>> {
            self.inner.capture_registration()
        }
        fn restore_registration(&self, bytes: &[u8]) -> io::Result<()> {
            self.inner.restore_registration(bytes)
        }
        fn create_shortcut(&self, link: &Path, target: &Path, description: &str) -> io::Result<()> {
            self.inner.create_shortcut(link, target, description)
        }
        fn shortcut_points_to(&self, link: &Path, target: &Path) -> io::Result<bool> {
            self.inner.shortcut_points_to(link, target)
        }
        fn write_uninstall_entry(&self, entry: &UninstallEntry) -> io::Result<()> {
            self.inner.write_uninstall_entry(entry)?;
            if self.crash {
                std::process::exit(73);
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
    let system = CrashAfterRegistration {
        inner: &system,
        crash: phase == "registration",
    };
    let new = payload::pack(&[("prism.exe", b"new"), ("res/new.txt", b"new resource")]).unwrap();
    install::install(
        &system,
        &new,
        "0.2.0",
        &options(&app),
        &setup,
        |stage, _| {
            if (phase == "preparing" && stage == Stage::Copying)
                || (phase == "files" && stage == Stage::Shortcuts)
                || (phase == "completed" && stage == Stage::Done)
            {
                std::process::exit(73);
            }
        },
    )
    .unwrap();
    panic!("Fixture did not reach its requested interruption point");
}

#[test]
#[cfg(windows)]
fn actual_process_exit_keeps_a_recoverable_or_committed_installation() {
    use std::os::windows::process::CommandExt;
    for phase in ["preparing", "files", "registration", "completed"] {
        let root = tempfile::Builder::new()
            .prefix("prism-recovery-test-")
            .tempdir()
            .unwrap();
        fs::write(
            root.path().join("fixture-owner.txt"),
            b"Prism isolated recovery fixture",
        )
        .unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "recovery_tests::crash_fixture_process",
                "--ignored",
                "--nocapture",
            ])
            .env("PRISM_RECOVERY_TEST_ROOT", root.path())
            .env("PRISM_RECOVERY_TEST_PHASE", phase)
            .creation_flags(0x0800_0000)
            .output()
            .unwrap();
        assert_eq!(
            status.status.code(),
            Some(73),
            "{phase}: {}",
            String::from_utf8_lossy(&status.stderr)
        );
        let system = crate::system::SandboxSystem {
            root: root.path().into(),
        };
        let app = root.path().join("app");
        let lock = crate::folder_lock::FolderLock::acquire(&app).unwrap();
        let count = recovery::recover(&system, &app).unwrap();
        assert_eq!(
            count,
            usize::from(matches!(phase, "files" | "registration"))
        );
        let expected: &[u8] = if phase == "completed" { b"new" } else { b"old" };
        assert_eq!(
            fs::read(app.join("prism.exe")).unwrap(),
            expected,
            "{phase}"
        );
        assert_eq!(fs::read(app.join("personal.txt")).unwrap(), b"user file");
        assert_eq!(
            system.existing_install().unwrap().1.as_deref(),
            Some(if phase == "completed" {
                "0.2.0"
            } else {
                "0.1.0"
            })
        );
        assert_eq!(recovery::recover(&system, &app).unwrap(), 0);
        assert!(!recovery::has_workspaces(&app));
        drop(lock);
    }
}
