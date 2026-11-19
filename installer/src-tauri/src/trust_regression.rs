//! Fault injection never opens a Windows certificate store or imports a certificate.
use crate::{
    install::{self, tests::FakeSystem, InstallOptions, System},
    maintenance_guard, payload,
};
use std::{cell::Cell, fs};

#[cfg(windows)]
mod certificate_faults {
    use super::*;
    use crate::trust::{self, AddResult, CertStores, Intent, Pending};
    use std::{
        cell::RefCell,
        collections::{BTreeMap, BTreeSet},
        io,
    };

    const ID: &str = "in-memory-public-certificate";
    #[derive(Default)]
    struct State {
        entries: BTreeSet<(String, String)>,
        owned: BTreeMap<String, Vec<String>>,
        pending: Pending,
    }
    #[derive(Default)]
    struct FaultStores {
        state: RefCell<State>,
        marker_calls: Cell<usize>,
        fail_marker_at: Cell<Option<usize>>,
        fail_intent: Cell<bool>,
        fail_clear: Cell<bool>,
        concurrent_it_add: Cell<bool>,
        find_error: Cell<bool>,
        decline_remove: Cell<bool>,
        adds: Cell<usize>,
        removes: Cell<usize>,
    }
    impl FaultStores {
        fn present(&self, store: &str) -> bool {
            self.state
                .borrow()
                .entries
                .contains(&(store.into(), ID.into()))
        }
        fn own(&self, stores: &[&str]) {
            self.state
                .borrow_mut()
                .owned
                .insert(ID.into(), stores.iter().map(|s| (*s).into()).collect());
            for store in stores {
                self.state
                    .borrow_mut()
                    .entries
                    .insert(((*store).into(), ID.into()));
            }
        }
    }
    impl CertStores for FaultStores {
        fn id(&self, _: &[u8]) -> io::Result<String> {
            Ok(ID.into())
        }
        fn contains(&self, store: &str, _: &[u8]) -> io::Result<bool> {
            Ok(self.present(store))
        }
        fn add(&self, store: &str, _: &[u8]) -> io::Result<AddResult> {
            self.adds.set(self.adds.get() + 1);
            assert_eq!(self.state.borrow().pending[ID][store], Intent::Adding);
            if self.concurrent_it_add.replace(false) {
                self.state
                    .borrow_mut()
                    .entries
                    .insert((store.into(), ID.into()));
                return Ok(AddResult::AlreadyPresent);
            }
            Ok(
                if self
                    .state
                    .borrow_mut()
                    .entries
                    .insert((store.into(), ID.into()))
                {
                    AddResult::Created
                } else {
                    AddResult::AlreadyPresent
                },
            )
        }
        fn remove(&self, store: &str, _: &str) -> io::Result<bool> {
            self.removes.set(self.removes.get() + 1);
            assert_eq!(self.state.borrow().pending[ID][store], Intent::Removing);
            if self.find_error.get() {
                return Err(io::Error::other("injected certificate lookup failure"));
            }
            if self.decline_remove.get() {
                return Ok(false);
            }
            self.state
                .borrow_mut()
                .entries
                .remove(&(store.into(), ID.into()));
            Ok(true)
        }
        fn added(&self) -> io::Result<BTreeMap<String, Vec<String>>> {
            Ok(self.state.borrow().owned.clone())
        }
        fn set_added(&self, id: &str, stores: &[String]) -> io::Result<()> {
            let call = self.marker_calls.get() + 1;
            self.marker_calls.set(call);
            if self.fail_marker_at.get() == Some(call) {
                return Err(io::Error::other("injected ownership persistence failure"));
            }
            let mut state = self.state.borrow_mut();
            if stores.is_empty() {
                state.owned.remove(id);
            } else {
                state.owned.insert(id.into(), stores.to_vec());
            }
            Ok(())
        }
        fn pending(&self) -> io::Result<Pending> {
            Ok(self.state.borrow().pending.clone())
        }
        fn set_pending(&self, id: &str, store: &str, intent: Option<Intent>) -> io::Result<()> {
            if (intent.is_some() && self.fail_intent.get())
                || (intent.is_none() && self.fail_clear.get())
            {
                return Err(io::Error::other("injected intent persistence failure"));
            }
            let mut state = self.state.borrow_mut();
            if let Some(intent) = intent {
                state
                    .pending
                    .entry(id.into())
                    .or_default()
                    .insert(store.into(), intent);
            } else if let Some(entries) = state.pending.get_mut(id) {
                entries.remove(store);
                if entries.is_empty() {
                    state.pending.remove(id);
                }
            }
            Ok(())
        }
    }
    fn cert() -> Vec<u8> {
        trust::tests::public_code_certificate()
    }

    #[test]
    fn intent_write_failure_prevents_both_directions_of_store_mutation() {
        let stores = FaultStores::default();
        stores.fail_intent.set(true);
        assert!(trust::trust(&stores, &cert(), true).is_err());
        assert_eq!(stores.adds.get(), 0);
        stores.own(&["Root", "TrustedPublisher"]);
        assert!(trust::untrust(&stores, true).is_err());
        assert_eq!(stores.removes.get(), 0);
        assert!(stores.present("Root") && stores.present("TrustedPublisher"));
    }

    #[test]
    fn add_ownership_failure_keeps_a_durable_receipt_and_never_claims_or_replays_it() {
        for fail_at in [1, 2] {
            let stores = FaultStores::default();
            stores.fail_marker_at.set(Some(fail_at));
            assert!(trust::trust(&stores, &cert(), true).is_err());
            let pending = stores.pending().unwrap();
            let affected = if fail_at == 1 {
                "Root"
            } else {
                "TrustedPublisher"
            };
            assert_eq!(pending[ID][affected], Intent::Adding);
            assert!(stores.present(affected));
            let adds = stores.adds.get();
            stores.fail_marker_at.set(None);
            assert!(trust::trust(&stores, &cert(), true).is_err());
            assert!(trust::untrust(&stores, true).is_err());
            assert_eq!(stores.adds.get(), adds);
            assert_eq!(stores.removes.get(), 0);
            assert!(stores.present("Root"));
        }
    }

    #[test]
    fn concurrent_it_add_is_not_claimed_and_survives_uninstall() {
        let stores = FaultStores::default();
        stores.concurrent_it_add.set(true);
        trust::trust(&stores, &cert(), true).unwrap();
        assert_eq!(stores.added().unwrap()[ID], ["TrustedPublisher"]);
        trust::untrust(&stores, true).unwrap();
        assert!(stores.present("Root"));
        assert!(!stores.present("TrustedPublisher"));
    }

    #[test]
    fn removal_then_marker_failure_then_it_readd_is_never_deleted_on_retry() {
        let stores = FaultStores::default();
        stores.own(&["Root", "TrustedPublisher"]);
        stores.fail_marker_at.set(Some(1));
        assert!(trust::untrust(&stores, true).is_err());
        assert!(!stores.present("TrustedPublisher"));
        assert!(stores.present("Root")); // Never remove its root after a publisher failure.
        assert_eq!(
            stores.pending().unwrap()[ID]["TrustedPublisher"],
            Intent::Removing
        );
        stores
            .state
            .borrow_mut()
            .entries
            .insert(("TrustedPublisher".into(), ID.into())); // IT re-add.
        stores.fail_marker_at.set(None);
        assert!(trust::untrust(&stores, true).is_err());
        assert_eq!(stores.removes.get(), 1);
        assert!(stores.present("TrustedPublisher") && stores.present("Root"));
    }

    #[test]
    fn intent_clear_failure_and_lookup_failure_preserve_explicit_pending_state() {
        for lookup in [false, true] {
            let stores = FaultStores::default();
            stores.own(&["Root", "TrustedPublisher"]);
            stores.fail_clear.set(!lookup);
            stores.find_error.set(lookup);
            assert!(trust::untrust(&stores, true).is_err());
            assert_eq!(
                stores.pending().unwrap()[ID]["TrustedPublisher"],
                Intent::Removing
            );
            assert!(stores.present("Root"));
            let removals = stores.removes.get();
            stores.fail_clear.set(false);
            stores.find_error.set(false);
            assert!(trust::untrust(&stores, true).is_err());
            assert_eq!(stores.removes.get(), removals);
        }
    }

    #[test]
    fn declined_publisher_removal_preserves_its_root_and_add_clear_failure_requires_review() {
        let stores = FaultStores::default();
        stores.own(&["Root", "TrustedPublisher"]);
        stores.decline_remove.set(true);
        assert!(trust::untrust(&stores, true).is_err());
        assert_eq!(stores.removes.get(), 1);
        assert!(stores.present("Root") && stores.present("TrustedPublisher"));
        assert!(stores.pending().unwrap().is_empty());

        let stores = FaultStores::default();
        stores.fail_clear.set(true);
        assert!(trust::trust(&stores, &cert(), true).is_err());
        assert_eq!(stores.pending().unwrap()[ID]["Root"], Intent::Adding);
        assert!(trust::untrust(&stores, true)
            .unwrap_err()
            .to_string()
            .starts_with("PENDING_REVIEW:"));
        assert_eq!(stores.removes.get(), 0);
    }

    #[test]
    fn malformed_existing_sandbox_state_is_preserved_before_any_mutation() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("trust.json");
        fs::write(&file, b"{ malformed existing state").unwrap();
        let stores = trust::FileStores {
            file,
            decline_root: false,
        };
        assert!(trust::trust(&stores, &cert(), true).is_err());
        assert_eq!(
            fs::read(stores.file).unwrap(),
            b"{ malformed existing state"
        );
    }

    #[test]
    fn certificate_lookup_only_treats_crypt_e_not_found_as_absent() {
        use windows::Win32::Foundation::CRYPT_E_NOT_FOUND;
        assert!(trust::windows_store::check_find_error(CRYPT_E_NOT_FOUND.0 as u32).is_ok());
        for code in [0, 5, 87, 0x80070005] {
            assert_eq!(
                trust::windows_store::check_find_error(code)
                    .unwrap_err()
                    .raw_os_error(),
                Some(code as i32)
            );
        }
    }

    #[test]
    fn failed_atomic_sandbox_replace_cannot_truncate_a_durable_intent() {
        use std::os::windows::fs::OpenOptionsExt;
        let temp = tempfile::tempdir().unwrap();
        let stores = trust::FileStores {
            file: temp.path().join("trust.json"),
            decline_root: false,
        };
        stores
            .set_pending(ID, "Root", Some(Intent::Adding))
            .unwrap();
        let before = fs::read(&stores.file).unwrap();
        let held = fs::OpenOptions::new()
            .read(true)
            .share_mode(0x1 | 0x2)
            .open(&stores.file)
            .unwrap();
        assert!(stores.set_added(ID, &["Root".into()]).is_err());
        drop(held);
        assert_eq!(fs::read(&stores.file).unwrap(), before);
        assert_eq!(stores.pending().unwrap()[ID]["Root"], Intent::Adding);
    }
}

fn options(path: &std::path::Path) -> InstallOptions {
    InstallOptions {
        install_dir: path.into(),
        desktop_shortcut: false,
        start_menu_shortcut: false,
    }
}

#[test]
fn install_and_uninstall_completion_keep_the_single_global_guard_and_strict_owner_receipt() {
    let temp = tempfile::tempdir().unwrap();
    let system = FakeSystem::new(temp.path());
    let setup = temp.path().join("inert-setup.exe");
    fs::write(&setup, b"never executed").unwrap();
    let app = temp.path().join("app");
    let bytes = payload::pack(&[("prism.exe", b"inert app")]).unwrap();
    let called = Cell::new(false);
    install::install_with_completion(
        &system,
        &bytes,
        "0.1.0",
        &options(&app),
        &setup,
        |_, _| {},
        |system| {
            assert!(maintenance_guard::acquire(system).is_err());
            assert!(maintenance_guard::registered_owner(system, &app).unwrap());
            called.set(true);
        },
    )
    .unwrap();
    assert!(called.replace(false));
    install::uninstall_with_completion(
        &system,
        &app,
        false,
        |_, _| {},
        |system, receipt| {
            assert!(maintenance_guard::acquire(system).is_err());
            assert!(receipt.may_untrust());
            assert!(system.checked_existing_install().unwrap().is_none());
            called.set(true);
        },
    )
    .unwrap();
    assert!(called.get());
}

#[test]
fn orphan_without_registration_never_receives_shared_certificate_removal_authority() {
    let temp = tempfile::tempdir().unwrap();
    let system = FakeSystem::new(temp.path());
    let setup = temp.path().join("inert-setup.exe");
    fs::write(&setup, b"never executed").unwrap();
    let app = temp.path().join("app");
    let bytes = payload::pack(&[("prism.exe", b"inert app")]).unwrap();
    install::install(&system, &bytes, "0.1.0", &options(&app), &setup, |_, _| {}).unwrap();
    *system.entry.borrow_mut() = None;
    let called = Cell::new(false);
    install::uninstall_with_completion(
        &system,
        &app,
        false,
        |_, _| {},
        |_, receipt| {
            assert!(!receipt.may_untrust());
            called.set(true);
        },
    )
    .unwrap();
    assert!(called.get());
}

#[test]
fn changed_registration_suppresses_install_certificate_completion() {
    let temp = tempfile::tempdir().unwrap();
    let system = FakeSystem::new(temp.path());
    let setup = temp.path().join("inert-setup.exe");
    fs::write(&setup, b"never executed").unwrap();
    let app = temp.path().join("app A");
    let other = temp.path().join("app B");
    fs::create_dir(&other).unwrap();
    let bytes = payload::pack(&[("prism.exe", b"inert app")]).unwrap();
    let called = Cell::new(false);
    let result = install::install_with_completion(
        &system,
        &bytes,
        "0.1.0",
        &options(&app),
        &setup,
        |stage, _| {
            if stage == install::Stage::Done {
                system.entry.borrow_mut().as_mut().unwrap().install_dir = other.clone();
            }
        },
        |_| called.set(true),
    );
    assert!(result.unwrap_err().starts_with("FOREIGN_INSTALLATION:"));
    assert!(!called.get());
}

struct CheckedFault<'a> {
    inner: &'a FakeSystem,
    fail: Cell<bool>,
}
impl System for CheckedFault<'_> {
    fn folders(&self) -> std::io::Result<install::Folders> {
        self.inner.folders()
    }
    fn capture_registration(&self) -> std::io::Result<Vec<u8>> {
        self.inner.capture_registration()
    }
    fn restore_registration(&self, bytes: &[u8]) -> std::io::Result<()> {
        self.inner.restore_registration(bytes)
    }
    fn create_shortcut(
        &self,
        link: &std::path::Path,
        target: &std::path::Path,
        description: &str,
    ) -> std::io::Result<()> {
        self.inner.create_shortcut(link, target, description)
    }
    fn shortcut_points_to(
        &self,
        link: &std::path::Path,
        target: &std::path::Path,
    ) -> std::io::Result<bool> {
        self.inner.shortcut_points_to(link, target)
    }
    fn write_uninstall_entry(&self, entry: &install::UninstallEntry) -> std::io::Result<()> {
        self.inner.write_uninstall_entry(entry)
    }
    fn remove_uninstall_entry(&self) -> std::io::Result<()> {
        self.inner.remove_uninstall_entry()
    }
    fn existing_install(&self) -> Option<(std::path::PathBuf, Option<String>)> {
        self.inner.existing_install()
    }
    fn checked_existing_install(
        &self,
    ) -> std::io::Result<Option<(std::path::PathBuf, Option<String>)>> {
        if self.fail.get() {
            Err(std::io::Error::other(
                "injected checked registration I/O error",
            ))
        } else {
            self.inner.checked_existing_install()
        }
    }
}

#[test]
fn certificate_completion_fails_closed_on_post_operation_registration_io_errors() {
    for remove in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let system = FakeSystem::new(temp.path());
        let setup = temp.path().join("inert-setup.exe");
        fs::write(&setup, b"never executed").unwrap();
        let app = temp.path().join("app");
        let bytes = payload::pack(&[("prism.exe", b"inert app")]).unwrap();
        let checked = CheckedFault {
            inner: &system,
            fail: Cell::new(false),
        };
        let called = Cell::new(false);
        let progress = |stage, _| {
            if stage == install::Stage::Done {
                checked.fail.set(true);
            }
        };
        let result = if remove {
            install::install(&system, &bytes, "0.1.0", &options(&app), &setup, |_, _| {}).unwrap();
            install::uninstall_with_completion(&checked, &app, false, progress, |_, _| {
                called.set(true)
            })
        } else {
            install::install_with_completion(
                &checked,
                &bytes,
                "0.1.0",
                &options(&app),
                &setup,
                progress,
                |_| called.set(true),
            )
        };
        assert!(result
            .unwrap_err()
            .contains("checked registration I/O error"));
        assert!(!called.get());
    }
}
