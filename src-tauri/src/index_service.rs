//! Read-only status plus explicit, serialized administrative actions. Service
//! changes are never called from startup or from an isolated test profile.
use serde::{Deserialize, Serialize};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

#[cfg(windows)]
#[path = "index_service_native.rs"]
mod native;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Action {
    Install,
    Start,
    Uninstall,
}
impl Action {
    #[cfg(windows)]
    fn argument(self) -> &'static str {
        match self {
            Self::Install => "--install",
            Self::Start => "--start",
            Self::Uninstall => "--uninstall",
        }
    }
}
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Issue {
    Unsupported,
    Isolated,
    UnsafeLocation,
    ExistingBinary,
    UnrecognizedService,
    Unavailable,
    Busy,
    Cancelled,
    PermissionDenied,
    TimedOut,
    Disabled,
    Failed,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Error {
    pub code: Issue,
    pub exit_code: Option<u32>,
}
impl From<Issue> for Error {
    fn from(code: Issue) -> Self {
        Self {
            code,
            exit_code: None,
        }
    }
}
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ServiceState {
    Unsupported,
    NotInstalled,
    Stopped,
    Running,
    Unavailable,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub state: ServiceState,
    pub issue: Option<Issue>,
    pub management_allowed: bool,
    pub busy: bool,
    pub can_install: bool,
    pub can_start: bool,
    pub can_remove: bool,
}
impl Status {
    fn permits(&self, action: Action) -> bool {
        match action {
            Action::Install => self.can_install,
            Action::Start => self.can_start,
            Action::Uninstall => self.can_remove,
        }
    }
}
#[derive(Clone, Default)]
pub struct Controller {
    busy: Arc<AtomicBool>,
}
struct BusyGuard(Arc<AtomicBool>);
impl Drop for BusyGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
impl Controller {
    pub fn status(&self, allowed: bool) -> Status {
        status(allowed, self.busy.load(Ordering::Acquire))
    }
    pub fn perform(&self, action: Action, allowed: bool) -> Result<Status, Error> {
        // This gate precedes even staging an executable or requesting UAC.
        if !allowed {
            return Err(Issue::Isolated.into());
        }
        if !cfg!(windows) {
            return Err(Issue::Unsupported.into());
        }
        if self
            .busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(Issue::Busy.into());
        }
        let guard = BusyGuard(self.busy.clone());
        // Ignore only this call's own busy flag when repeating preflight.
        let before = status(allowed, false);
        if !before.permits(action) {
            return Err(before.issue.unwrap_or(Issue::Unavailable).into());
        }
        #[cfg(windows)]
        native::launch(action)?;
        drop(guard);
        Ok(self.status(allowed))
    }
}
fn status(allowed: bool, busy: bool) -> Status {
    let mut result = Status {
        state: ServiceState::Unsupported,
        issue: Some(Issue::Unsupported),
        management_allowed: allowed,
        busy,
        can_install: false,
        can_start: false,
        can_remove: false,
    };
    #[cfg(windows)]
    {
        use prism_ntfs_windows::{administration, scm};
        result.state = match scm::state() {
            Ok(scm::State::NotInstalled) => ServiceState::NotInstalled,
            Ok(scm::State::Stopped) => ServiceState::Stopped,
            Ok(scm::State::Running) => ServiceState::Running,
            Err(_) => ServiceState::Unavailable,
        };
        result.issue = match result.state {
            ServiceState::NotInstalled => {
                administration::check_install_location()
                    .err()
                    .map(|error| match error.kind() {
                        std::io::ErrorKind::PermissionDenied => Issue::UnsafeLocation,
                        std::io::ErrorKind::AlreadyExists => Issue::ExistingBinary,
                        _ => Issue::Unavailable,
                    })
            }
            ServiceState::Stopped | ServiceState::Running => administration::check_owned_service()
                .err()
                .map(|_| Issue::UnrecognizedService),
            _ => Some(Issue::Unavailable),
        };
        if allowed && !busy && result.issue.is_none() {
            result.can_install = result.state == ServiceState::NotInstalled;
            result.can_start = result.state == ServiceState::Stopped;
            result.can_remove =
                matches!(result.state, ServiceState::Stopped | ServiceState::Running);
        }
    }
    // Mode is reported independently from an actual OS/preflight failure.
    result.management_allowed = allowed && cfg!(windows);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[cfg(not(windows))]
    fn other_platforms_keep_service_management_unavailable() {
        let controller = Controller::default();
        let status = controller.status(true);
        assert_eq!(status.state, ServiceState::Unsupported);
        assert!(
            !status.management_allowed
                && !status.can_install
                && !status.can_start
                && !status.can_remove
        );
        assert_eq!(
            controller.perform(Action::Install, true).unwrap_err().code,
            Issue::Unsupported
        );
    }
    #[test]
    fn isolated_mode_rejects_every_action_before_os_work() {
        let controller = Controller::default();
        for action in [Action::Install, Action::Start, Action::Uninstall] {
            assert_eq!(
                controller.perform(action, false).unwrap_err().code,
                Issue::Isolated
            );
            assert!(!controller.busy.load(Ordering::Acquire));
        }
    }
    #[test]
    fn gate_release_and_action_serialization_do_not_depend_on_ui_lifetime() {
        let controller = Controller::default();
        controller.busy.store(true, Ordering::Release);
        let guard = BusyGuard(controller.busy.clone());
        #[cfg(windows)]
        assert_eq!(
            controller.perform(Action::Install, true).unwrap_err().code,
            Issue::Busy
        );
        drop(guard);
        assert!(!controller.busy.load(Ordering::Acquire));
        let guard = BusyGuard(controller.busy.clone());
        controller.busy.store(true, Ordering::Release);
        let _ = std::panic::catch_unwind(|| {
            let _guard = guard;
            panic!("unit-only unwind");
        });
        assert!(!controller.busy.load(Ordering::Acquire));
    }
}
