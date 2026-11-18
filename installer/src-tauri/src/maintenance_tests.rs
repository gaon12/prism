//! Maintenance failures use only inert payloads, fake registration and temporary folders.
use crate::{
    install::{self, tests::FakeSystem, Folders, InstallOptions, System, UninstallEntry},
    maintenance_guard, payload, previous,
    transaction::Files,
};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

struct Closing<'a> {
    inner: &'a FakeSystem,
    fail: bool,
    calls: RefCell<Vec<PathBuf>>,
    fail_verify_at: Option<usize>,
    verify_calls: Cell<usize>,
}
impl System for Closing<'_> {
    fn prepare_app_for_maintenance(&self, app: &Path) -> io::Result<()> {
        self.calls.borrow_mut().push(app.into());
        if self.fail {
            Err(io::Error::other(
                "APP_CLOSE_FAILED: synthetic close refusal",
            ))
        } else {
            Ok(())
        }
    }
    fn verify_app_stopped(&self, _app: &Path) -> io::Result<()> {
        let call = self.verify_calls.get() + 1;
        self.verify_calls.set(call);
        if self.fail_verify_at == Some(call) {
            Err(io::Error::other(
                "APP_RUNNING: a new instance appeared after staging",
            ))
        } else {
            Ok(())
        }
    }
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
        self.inner.write_uninstall_entry(entry)
    }
    fn remove_uninstall_entry(&self) -> io::Result<()> {
        self.inner.remove_uninstall_entry()
    }
    fn existing_install(&self) -> Option<(PathBuf, Option<String>)> {
        self.inner.existing_install()
    }
    fn login_startup(&self) -> io::Result<Option<String>> {
        self.inner.login_startup()
    }
    fn remove_login_startup(&self) -> io::Result<()> {
        self.inner.remove_login_startup()
    }
}

fn options(app: &Path) -> InstallOptions {
    InstallOptions {
        install_dir: app.into(),
        desktop_shortcut: false,
        start_menu_shortcut: false,
    }
}

fn fixture(root: &Path) -> (FakeSystem, PathBuf, PathBuf) {
    let system = FakeSystem::new(root);
    let app = root.join("installation A");
    let setup = root.join("setup.exe");
    fs::write(&setup, b"inert setup bytes").unwrap();
    let bytes = payload::pack(&[("prism.exe", b"old app"), ("res.txt", b"old resource")]).unwrap();
    install::install(&system, &bytes, "0.1.0", &options(&app), &setup, |_, _| {}).unwrap();
    fs::write(app.join("personal.txt"), b"private canary").unwrap();
    for base in [root.join("Local"), root.join("Roaming")] {
        let profile = base.join(install::APP_IDENTIFIER);
        fs::create_dir_all(&profile).unwrap();
        fs::write(profile.join("personal.txt"), b"shared profile canary").unwrap();
    }
    (system, app, setup)
}

fn pending(system: &FakeSystem, app: &Path) {
    let mut files = Files::new(app).unwrap();
    files.snapshot(app, "prism.exe").unwrap();
    files.snapshot(app, install::RECORD_FILE).unwrap();
    fs::write(
        files.directory.join("registration.json"),
        system.capture_registration().unwrap(),
    )
    .unwrap();
    files.begin_changes().unwrap();
    fs::write(app.join("prism.exe"), b"interrupted app").unwrap();
    files.retain();
}

fn tree(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn collect(root: &Path, dir: &Path, result: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for item in fs::read_dir(dir).unwrap() {
            let item = item.unwrap();
            assert!(!item.file_type().unwrap().is_symlink());
            if item.file_type().unwrap().is_dir() {
                collect(root, &item.path(), result);
            } else {
                result.insert(
                    item.path().strip_prefix(root).unwrap().into(),
                    fs::read(item.path()).unwrap(),
                );
            }
        }
    }
    let mut result = BTreeMap::new();
    collect(root, root, &mut result);
    result
}

fn operation(kind: u8, system: &dyn System, app: &Path, setup: &Path) -> Result<(), String> {
    let bytes = payload::pack(&[("prism.exe", b"candidate app")]).unwrap();
    match kind {
        0 => install::install(system, &bytes, "0.2.0", &options(app), setup, |_, _| {}),
        1 => install::repair(system, &bytes, "0.1.0", &options(app), setup, |_, _| {}),
        2 => install::uninstall(system, app, false, |_, _| {}),
        3 => previous::restore(system, app, "0.0.9", |_, _| {}),
        _ => unreachable!(),
    }
}

#[test]
fn close_refusal_precedes_recovery_and_every_maintenance_mutation() {
    for kind in 0..4 {
        let temp = tempfile::tempdir().unwrap();
        let (system, app, setup) = fixture(temp.path());
        pending(&system, &app);
        let before = tree(temp.path());
        let registration = system.capture_registration().unwrap();
        let closing = Closing {
            inner: &system,
            fail: true,
            calls: RefCell::new(Vec::new()),
            fail_verify_at: None,
            verify_calls: Cell::new(0),
        };
        let error = operation(kind, &closing, &app, &setup).unwrap_err();
        assert!(
            error.starts_with("APP_CLOSE_FAILED:"),
            "operation {kind}: {error}"
        );
        assert_eq!(closing.calls.borrow().len(), 1);
        assert!(crate::safe_path::same_target(
            &closing.calls.borrow()[0],
            &app.join(install::APP_EXE)
        ));
        assert_eq!(tree(temp.path()), before, "operation {kind}");
        assert_eq!(system.capture_registration().unwrap(), registration);
    }
}

#[test]
fn foreign_registered_installation_preserves_files_profiles_and_registration() {
    for kind in 0..4 {
        let temp = tempfile::tempdir().unwrap();
        let (system, app, setup) = fixture(temp.path());
        pending(&system, &app);
        let other = temp.path().join("installation B");
        fs::create_dir(&other).unwrap();
        fs::write(other.join("prism.exe"), b"registered other app").unwrap();
        system
            .write_uninstall_entry(&UninstallEntry {
                version: "0.3.0".into(),
                install_dir: other,
                size_kib: 1,
            })
            .unwrap();
        let before = tree(temp.path());
        let registration = system.capture_registration().unwrap();
        let closing = Closing {
            inner: &system,
            fail: false,
            calls: RefCell::new(Vec::new()),
            fail_verify_at: None,
            verify_calls: Cell::new(0),
        };
        assert_eq!(
            operation(kind, &closing, &app, &setup),
            Err(maintenance_guard::FOREIGN_INSTALLATION.into())
        );
        assert!(closing.calls.borrow().is_empty());
        assert_eq!(tree(temp.path()), before, "operation {kind}");
        assert_eq!(system.capture_registration().unwrap(), registration);
    }
}

#[test]
fn absent_registration_cannot_authorize_profile_deletion_or_recovery_writes() {
    let temp = tempfile::tempdir().unwrap();
    let (system, app, _) = fixture(temp.path());
    pending(&system, &app);
    *system.entry.borrow_mut() = None;
    let before = tree(temp.path());
    let closing = Closing {
        inner: &system,
        fail: false,
        calls: RefCell::new(Vec::new()),
        fail_verify_at: None,
        verify_calls: Cell::new(0),
    };
    assert!(install::uninstall(&closing, &app, true, |_, _| {}).is_err());
    assert!(closing.calls.borrow().is_empty());
    assert_eq!(tree(temp.path()), before);
    assert!(system.entry.borrow().is_none());
}

#[test]
fn global_contention_precedes_target_creation_close_or_recovery() {
    let temp = tempfile::tempdir().unwrap();
    let (system, app, setup) = fixture(temp.path());
    pending(&system, &app);
    let other = temp.path().join("installation B");
    let before = tree(temp.path());
    let registration = system.capture_registration().unwrap();
    let closing = Closing {
        inner: &system,
        fail: false,
        calls: RefCell::new(Vec::new()),
        fail_verify_at: None,
        verify_calls: Cell::new(0),
    };
    let guard = maintenance_guard::acquire(&system).unwrap();
    for kind in 0..4 {
        let target = if kind == 0 { &other } else { &app };
        let error = operation(kind, &closing, target, &setup).unwrap_err();
        assert!(error.contains("Another setup"), "operation {kind}: {error}");
    }
    assert!(closing.calls.borrow().is_empty());
    assert!(!other.exists());
    // Windows byte-range locking also denies reading the held lock file itself.
    drop(guard);
    assert_eq!(tree(temp.path()), before);
    assert_eq!(system.capture_registration().unwrap(), registration);
    assert!(maintenance_guard::acquire(&system).is_ok());
}

#[cfg(windows)]
#[test]
fn locked_second_resource_blocks_uninstall_before_the_first_file_is_deleted() {
    use std::os::windows::fs::OpenOptionsExt;
    let temp = tempfile::tempdir().unwrap();
    let (system, app, _) = fixture(temp.path());
    let before = tree(temp.path());
    let registration = system.capture_registration().unwrap();
    let executable = fs::read(app.join(install::APP_EXE)).unwrap();
    let record = fs::read(app.join(install::RECORD_FILE)).unwrap();
    let held = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(app.join("res.txt"))
        .unwrap();
    assert!(install::uninstall(&system, &app, false, |_, _| {}).is_err());
    assert_eq!(fs::read(app.join(install::APP_EXE)).unwrap(), executable);
    assert_eq!(fs::read(app.join(install::RECORD_FILE)).unwrap(), record);
    assert_eq!(system.capture_registration().unwrap(), registration);
    drop(held);
    assert_eq!(tree(temp.path()), before);
}

#[test]
fn invalid_shared_profile_root_fails_before_any_installed_file_is_removed() {
    for base in ["Roaming", "Local"] {
        let temp = tempfile::tempdir().unwrap();
        let (system, app, _) = fixture(temp.path());
        let profile = temp.path().join(base).join(install::APP_IDENTIFIER);
        fs::remove_file(profile.join("personal.txt")).unwrap();
        fs::remove_dir(&profile).unwrap();
        fs::write(
            &profile,
            b"a personal file cannot be adopted as a data directory",
        )
        .unwrap();
        let before = tree(temp.path());
        let registration = system.capture_registration().unwrap();
        let error = install::uninstall(&system, &app, true, |_, _| {}).unwrap_err();
        assert!(error.contains("data folder is unsafe"), "{error}");
        assert_eq!(tree(temp.path()), before);
        assert_eq!(system.capture_registration().unwrap(), registration);
    }
}

#[cfg(windows)]
#[test]
fn old_and_new_executable_guards_survive_staging_and_publication() {
    let temp = tempfile::tempdir().unwrap();
    let (system, app, setup) = fixture(temp.path());
    let bytes = payload::pack(&[
        ("prism.exe", b"updated app"),
        ("res.txt", b"updated resource"),
    ])
    .unwrap();
    let mut old_guard_observed = false;
    let mut new_guard_observed = false;
    install::install(
        &system,
        &bytes,
        "0.2.0",
        &options(&app),
        &setup,
        |stage, _| {
            if stage == install::Stage::Copying && !old_guard_observed {
                assert!(fs::File::open(app.join(install::APP_EXE)).is_err());
                old_guard_observed = true;
            }
            if stage == install::Stage::Shortcuts {
                assert!(fs::File::open(app.join(install::APP_EXE)).is_err());
                new_guard_observed = true;
            }
        },
    )
    .unwrap();
    assert!(old_guard_observed && new_guard_observed);
    assert_eq!(
        fs::read(app.join(install::APP_EXE)).unwrap(),
        b"updated app"
    );
    assert_eq!(system.entry.borrow().as_ref().unwrap().version, "0.2.0");
    assert_eq!(previous::available(&app).unwrap().unwrap().version, "0.1.0");
}

#[test]
fn a_new_instance_after_staging_refuses_mutation_without_another_close_request() {
    let temp = tempfile::tempdir().unwrap();
    let (system, app, setup) = fixture(temp.path());
    let before = tree(temp.path());
    let registration = system.capture_registration().unwrap();
    let closing = Closing {
        inner: &system,
        fail: false,
        calls: RefCell::new(Vec::new()),
        fail_verify_at: Some(2),
        verify_calls: Cell::new(0),
    };
    let error = operation(0, &closing, &app, &setup).unwrap_err();
    assert!(error.starts_with("APP_RUNNING:"), "{error}");
    assert_eq!(closing.calls.borrow().len(), 1);
    assert_eq!(closing.verify_calls.get(), 2);
    assert_eq!(tree(temp.path()), before);
    assert_eq!(system.capture_registration().unwrap(), registration);
}

#[test]
fn uninstall_removes_owned_empty_resource_ancestors_and_keeps_unknown_nested_files() {
    let temp = tempfile::tempdir().unwrap();
    let (system, app, setup) = fixture(temp.path());
    let bytes = payload::pack(&[
        ("prism.exe", b"inert app"),
        ("res/a.txt", b"owned resource"),
        ("resources/empty/deep/a.txt", b"owned nested resource"),
        ("resources/empty/b.txt", b"another owned resource"),
    ])
    .unwrap();
    install::install(&system, &bytes, "0.1.0", &options(&app), &setup, |_, _| {}).unwrap();
    let user_folder = app.join("resources/user/nested");
    fs::create_dir_all(&user_folder).unwrap();
    fs::write(user_folder.join("unknown.txt"), b"unrecorded user resource").unwrap();

    install::uninstall(&system, &app, false, |_, _| {}).unwrap();

    assert!(!app.join("res").exists());
    assert!(!app.join("resources/empty/deep").exists());
    assert!(!app.join("resources/empty").exists());
    assert_eq!(
        fs::read(app.join("personal.txt")).unwrap(),
        b"private canary"
    );
    assert_eq!(
        fs::read(user_folder.join("unknown.txt")).unwrap(),
        b"unrecorded user resource"
    );
    for name in [
        install::APP_EXE,
        install::UNINSTALLER_EXE,
        install::RECORD_FILE,
    ] {
        assert!(!app.join(name).exists());
    }
    assert!(system.entry.borrow().is_none());
}
