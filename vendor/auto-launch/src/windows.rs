use crate::{AutoLaunch, Result, WindowsEnableMode};
use std::io;
use windows_registry::{Key, CURRENT_USER, LOCAL_MACHINE};
use windows_result::HRESULT;

const AL_REGKEY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run";
const TASK_MANAGER_OVERRIDE_REGKEY: &str =
    r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
const TASK_MANAGER_OVERRIDE_ENABLED_VALUE: [u8; 12] = [
    0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];
const E_ACCESSDENIED: HRESULT = HRESULT::from_win32(0x80070005_u32);
const E_FILENOTFOUND: HRESULT = HRESULT::from_win32(0x80070002_u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RegistryScope {
    CurrentUser,
    System,
}

impl RegistryScope {
    fn key(self) -> &'static Key {
        match self {
            Self::CurrentUser => CURRENT_USER,
            Self::System => LOCAL_MACHINE,
        }
    }
}

// Keep scope selection shared by the real adapter and pure registry regressions.
// Explicit modes must never inspect or mutate the other user's/machine's entry.
fn disable_in_mode(
    mode: WindowsEnableMode,
    mut disable: impl FnMut(RegistryScope) -> windows_registry::Result<()>,
) -> windows_registry::Result<()> {
    match mode {
        WindowsEnableMode::CurrentUser => disable(RegistryScope::CurrentUser),
        WindowsEnableMode::System => disable(RegistryScope::System),
        WindowsEnableMode::Dynamic => {
            // Preserve the legacy Dynamic behavior: attempt both roots, ignoring
            // only denied machine access before removing the current-user entry.
            if let Err(error) = disable(RegistryScope::System) {
                if error.code() != E_ACCESSDENIED {
                    return Err(error);
                }
            }
            disable(RegistryScope::CurrentUser)
        }
    }
}

fn is_enabled_in_mode(
    mode: WindowsEnableMode,
    mut registered: impl FnMut(RegistryScope) -> io::Result<bool>,
    mut approved: impl FnMut(RegistryScope) -> io::Result<bool>,
) -> io::Result<bool> {
    match mode {
        WindowsEnableMode::CurrentUser => {
            Ok(registered(RegistryScope::CurrentUser)? && approved(RegistryScope::CurrentUser)?)
        }
        WindowsEnableMode::System => {
            Ok(registered(RegistryScope::System)? && approved(RegistryScope::System)?)
        }
        WindowsEnableMode::Dynamic => {
            let registered =
                registered(RegistryScope::System)? || registered(RegistryScope::CurrentUser)?;
            if !registered {
                return Ok(false);
            }
            Ok(approved(RegistryScope::System)? && approved(RegistryScope::CurrentUser)?)
        }
    }
}

/// Windows implement
impl AutoLaunch {
    /// Create a new AutoLaunch instance
    /// - `app_name`: application name
    /// - `app_path`: application path
    /// - `enable_mode`: behavior of the enable feature
    /// - `args`: startup args passed to the binary
    ///
    /// ## Notes
    ///
    /// The parameters of `AutoLaunch::new` are different on each platform.
    pub fn new(
        app_name: &str,
        app_path: &str,
        enable_mode: WindowsEnableMode,
        args: &[impl AsRef<str>],
    ) -> AutoLaunch {
        AutoLaunch {
            app_name: app_name.into(),
            app_path: app_path.into(),
            enable_mode,
            args: args.iter().map(|s| s.as_ref().to_string()).collect(),
        }
    }

    /// Enable the AutoLaunch setting
    ///
    /// ## Errors
    ///
    /// - failed to open the registry key
    /// - failed to set value
    pub fn enable(&self) -> Result<()> {
        match self.enable_mode {
            WindowsEnableMode::Dynamic => self
                .enable_as_admin()
                .or_else(|e| {
                    if e.code() == E_ACCESSDENIED {
                        self.enable_as_current_user()
                    } else {
                        Err(e)
                    }
                })
                .map_err(std::io::Error::from)?,
            WindowsEnableMode::CurrentUser => self
                .enable_as_current_user()
                .map_err(std::io::Error::from)?,
            WindowsEnableMode::System => self.enable_as_admin().map_err(std::io::Error::from)?,
        }
        Ok(())
    }

    fn enable_as_admin(&self) -> windows_registry::Result<()> {
        self.enable_with_root_key(LOCAL_MACHINE)
    }

    fn enable_as_current_user(&self) -> windows_registry::Result<()> {
        self.enable_with_root_key(CURRENT_USER)
    }

    fn enable_with_root_key(&self, root_key: &Key) -> windows_registry::Result<()> {
        root_key.create(AL_REGKEY)?.set_string(
            &self.app_name,
            format!("{} {}", &self.app_path, &self.args.join(" ")),
        )?;

        match root_key
            .options()
            .write()
            .open(TASK_MANAGER_OVERRIDE_REGKEY)
        {
            Ok(key) => key.set_bytes(
                &self.app_name,
                windows_registry::Type::Bytes,
                &TASK_MANAGER_OVERRIDE_ENABLED_VALUE,
            )?,
            Err(error) if error.code() == E_FILENOTFOUND => {
                return Ok(());
            }
            Err(error) => {
                return Err(error);
            }
        }
        Ok(())
    }

    /// Disable the AutoLaunch setting
    ///
    /// ## Errors
    ///
    /// - failed to open the registry key
    /// - failed to delete value
    pub fn disable(&self) -> Result<()> {
        disable_in_mode(self.enable_mode, |scope| {
            self.disable_with_root_key(scope.key())
        })
        .map_err(std::io::Error::from)?;
        Ok(())
    }

    fn disable_with_root_key(&self, root_key: &Key) -> windows_registry::Result<()> {
        match root_key
            .options()
            .write()
            .open(AL_REGKEY)
            .and_then(|key| key.remove_value(&self.app_name))
        {
            Ok(_) => Ok(()),
            Err(error) if error.code() == E_FILENOTFOUND => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// Check whether the AutoLaunch setting is enabled
    pub fn is_enabled(&self) -> Result<bool> {
        Ok(is_enabled_in_mode(
            self.enable_mode,
            |scope| self.is_registered(scope.key()),
            |scope| self.is_task_manager_enabled(scope.key()),
        )?)
    }

    fn is_registered(&self, root_key: &Key) -> io::Result<bool> {
        let registered = match root_key
            .open(AL_REGKEY)
            .and_then(|key| key.get_string(&self.app_name))
        {
            Ok(_) => true,
            Err(error) if error.code() == E_FILENOTFOUND => false,
            Err(error) => {
                return Err(error.into());
            }
        };
        Ok(registered)
    }

    fn is_task_manager_enabled(&self, root_key: &Key) -> io::Result<bool> {
        let task_manager_enabled = match root_key
            .open(TASK_MANAGER_OVERRIDE_REGKEY)
            .and_then(|key| key.get_value(&self.app_name))
        {
            Ok(value) => last_eight_bytes_all_zeros(&value).unwrap_or(true),
            Err(error) if error.code() == E_FILENOTFOUND => true,
            Err(error) => {
                return Err(error.into());
            }
        };
        Ok(task_manager_enabled)
    }
}

fn last_eight_bytes_all_zeros(bytes: &[u8]) -> std::result::Result<bool, &str> {
    if bytes.len() < 8 {
        Err("Bytes too short")
    } else {
        Ok(bytes.iter().rev().take(8).all(|v| *v == 0u8))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    // A registry model, never the OS hives. The production functions above use
    // these same mode-routing helpers with the real Key adapter.
    struct RegistryFixture {
        user: RefCell<Option<Vec<u8>>>,
        system: RefCell<Option<Vec<u8>>>,
        user_approved: Cell<bool>,
        system_approved: Cell<bool>,
        failure: Cell<Option<(RegistryScope, HRESULT)>>,
        calls: RefCell<Vec<(&'static str, RegistryScope)>>,
    }

    impl RegistryFixture {
        fn new() -> Self {
            Self {
                user: RefCell::new(Some(b"user-owned startup command".to_vec())),
                system: RefCell::new(Some(b"machine-owned startup command".to_vec())),
                user_approved: Cell::new(true),
                system_approved: Cell::new(true),
                failure: Cell::new(None),
                calls: RefCell::new(Vec::new()),
            }
        }

        fn access(
            &self,
            operation: &'static str,
            scope: RegistryScope,
        ) -> windows_registry::Result<()> {
            self.calls.borrow_mut().push((operation, scope));
            if let Some((denied_scope, code)) = self.failure.get() {
                if denied_scope == scope {
                    return Err(code.into());
                }
            }
            Ok(())
        }

        fn remove(&self, scope: RegistryScope) -> windows_registry::Result<()> {
            self.access("remove", scope)?;
            match scope {
                RegistryScope::CurrentUser => self.user.borrow_mut().take(),
                RegistryScope::System => self.system.borrow_mut().take(),
            };
            Ok(())
        }

        fn registered(&self, scope: RegistryScope) -> io::Result<bool> {
            self.access("registered", scope).map_err(io::Error::from)?;
            Ok(match scope {
                RegistryScope::CurrentUser => self.user.borrow().is_some(),
                RegistryScope::System => self.system.borrow().is_some(),
            })
        }

        fn approved(&self, scope: RegistryScope) -> io::Result<bool> {
            self.access("approved", scope).map_err(io::Error::from)?;
            Ok(match scope {
                RegistryScope::CurrentUser => self.user_approved.get(),
                RegistryScope::System => self.system_approved.get(),
            })
        }

        fn status(&self, mode: WindowsEnableMode) -> io::Result<bool> {
            is_enabled_in_mode(
                mode,
                |scope| self.registered(scope),
                |scope| self.approved(scope),
            )
        }
    }

    #[test]
    fn current_user_disable_never_accesses_or_changes_machine_entry() {
        let registry = RegistryFixture::new();
        let machine_before = registry.system.borrow().clone();
        registry
            .failure
            .set(Some((RegistryScope::System, E_ACCESSDENIED)));
        disable_in_mode(WindowsEnableMode::CurrentUser, |scope| {
            registry.remove(scope)
        })
        .unwrap();
        assert!(registry.user.borrow().is_none());
        assert_eq!(*registry.system.borrow(), machine_before);
        assert_eq!(
            *registry.calls.borrow(),
            [("remove", RegistryScope::CurrentUser)]
        );
    }

    #[test]
    fn current_user_status_uses_only_its_own_registration_and_approval() {
        for approved in [true, false] {
            let registry = RegistryFixture::new();
            registry.user_approved.set(approved);
            registry.system_approved.set(!approved);
            registry
                .failure
                .set(Some((RegistryScope::System, E_ACCESSDENIED)));
            assert_eq!(
                registry.status(WindowsEnableMode::CurrentUser).unwrap(),
                approved
            );
            assert_eq!(
                *registry.calls.borrow(),
                [
                    ("registered", RegistryScope::CurrentUser),
                    ("approved", RegistryScope::CurrentUser),
                ]
            );
        }
    }

    #[test]
    fn current_user_is_disabled_when_only_the_machine_is_registered() {
        let registry = RegistryFixture::new();
        registry.user.replace(None);
        assert!(!registry.status(WindowsEnableMode::CurrentUser).unwrap());
        assert_eq!(
            *registry.calls.borrow(),
            [("registered", RegistryScope::CurrentUser)]
        );
    }

    #[test]
    fn system_disable_never_accesses_or_changes_current_user_entry() {
        let registry = RegistryFixture::new();
        let user_before = registry.user.borrow().clone();
        registry
            .failure
            .set(Some((RegistryScope::CurrentUser, E_ACCESSDENIED)));
        disable_in_mode(WindowsEnableMode::System, |scope| registry.remove(scope)).unwrap();
        assert!(registry.system.borrow().is_none());
        assert_eq!(*registry.user.borrow(), user_before);
        assert_eq!(
            *registry.calls.borrow(),
            [("remove", RegistryScope::System)]
        );
    }

    #[test]
    fn system_status_uses_only_machine_registration_and_approval() {
        for approved in [true, false] {
            let registry = RegistryFixture::new();
            registry.system_approved.set(approved);
            registry.user_approved.set(!approved);
            registry
                .failure
                .set(Some((RegistryScope::CurrentUser, E_ACCESSDENIED)));
            assert_eq!(
                registry.status(WindowsEnableMode::System).unwrap(),
                approved
            );
            assert_eq!(
                *registry.calls.borrow(),
                [
                    ("registered", RegistryScope::System),
                    ("approved", RegistryScope::System),
                ]
            );
        }
        let registry = RegistryFixture::new();
        registry.system.replace(None);
        assert!(!registry.status(WindowsEnableMode::System).unwrap());
        assert_eq!(
            *registry.calls.borrow(),
            [("registered", RegistryScope::System)]
        );
    }

    #[test]
    fn explicit_scope_failures_are_propagated_without_touching_the_other_scope() {
        for (mode, scope) in [
            (WindowsEnableMode::CurrentUser, RegistryScope::CurrentUser),
            (WindowsEnableMode::System, RegistryScope::System),
        ] {
            let registry = RegistryFixture::new();
            let before = (
                registry.user.borrow().clone(),
                registry.system.borrow().clone(),
            );
            registry.failure.set(Some((scope, E_ACCESSDENIED)));
            assert_eq!(
                disable_in_mode(mode, |root| registry.remove(root))
                    .unwrap_err()
                    .code(),
                E_ACCESSDENIED
            );
            assert_eq!(
                (
                    registry.user.borrow().clone(),
                    registry.system.borrow().clone()
                ),
                before
            );
            assert_eq!(*registry.calls.borrow(), [("remove", scope)]);
            registry.calls.borrow_mut().clear();
            assert!(registry.status(mode).is_err());
            assert_eq!(*registry.calls.borrow(), [("registered", scope)]);
        }
    }

    #[test]
    fn dynamic_disable_preserves_legacy_both_root_cleanup() {
        let registry = RegistryFixture::new();
        disable_in_mode(WindowsEnableMode::Dynamic, |root| registry.remove(root)).unwrap();
        assert!(registry.user.borrow().is_none() && registry.system.borrow().is_none());
        assert_eq!(
            *registry.calls.borrow(),
            [
                ("remove", RegistryScope::System),
                ("remove", RegistryScope::CurrentUser)
            ]
        );
    }

    #[test]
    fn dynamic_disable_falls_back_only_on_machine_access_denied() {
        for code in [E_ACCESSDENIED, HRESULT::from_win32(13)] {
            let registry = RegistryFixture::new();
            let machine_before = registry.system.borrow().clone();
            registry.failure.set(Some((RegistryScope::System, code)));
            let result = disable_in_mode(WindowsEnableMode::Dynamic, |root| registry.remove(root));
            assert_eq!(result.is_ok(), code == E_ACCESSDENIED);
            assert_eq!(*registry.system.borrow(), machine_before);
            if code == E_ACCESSDENIED {
                assert!(registry.user.borrow().is_none());
                assert_eq!(
                    *registry.calls.borrow(),
                    [
                        ("remove", RegistryScope::System),
                        ("remove", RegistryScope::CurrentUser)
                    ]
                );
            } else {
                assert!(registry.user.borrow().is_some());
                assert_eq!(
                    *registry.calls.borrow(),
                    [("remove", RegistryScope::System)]
                );
            }
        }
    }

    #[test]
    fn dynamic_status_preserves_legacy_cross_root_approval_and_short_circuiting() {
        let registry = RegistryFixture::new();
        registry.user_approved.set(false);
        assert!(!registry.status(WindowsEnableMode::Dynamic).unwrap());
        assert_eq!(
            *registry.calls.borrow(),
            [
                ("registered", RegistryScope::System),
                ("approved", RegistryScope::System),
                ("approved", RegistryScope::CurrentUser)
            ]
        );
        registry.system.replace(None);
        registry.user_approved.set(true);
        registry.calls.borrow_mut().clear();
        assert!(registry.status(WindowsEnableMode::Dynamic).unwrap());
        assert_eq!(
            *registry.calls.borrow(),
            [
                ("registered", RegistryScope::System),
                ("registered", RegistryScope::CurrentUser),
                ("approved", RegistryScope::System),
                ("approved", RegistryScope::CurrentUser)
            ]
        );
    }

    #[test]
    fn test_last_eight_bytes_all_zeros_true() {
        let data = [1u8, 2, 3, 4, 5, 0, 0, 0, 0, 0, 0, 0, 0];
        assert_eq!(last_eight_bytes_all_zeros(&data).unwrap(), true);
    }

    #[test]
    fn test_last_eight_bytes_all_zeros_false() {
        let data = [0u8, 0, 0, 0, 0, 0, 0, 1];
        assert_eq!(last_eight_bytes_all_zeros(&data).unwrap(), false);
    }

    #[test]
    fn test_last_eight_bytes_all_zeros_too_short() {
        let data = [0u8, 0, 0, 0, 0, 0, 0];
        assert!(last_eight_bytes_all_zeros(&data).is_err());
    }
}
