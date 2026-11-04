//! Packaged desktop apps use their manifest StartupTask, never an HKCU Run entry.
pub const TASK_ID: &str = "PrismStartup";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Disabled,
    DisabledByUser,
    Enabled,
    DisabledByPolicy,
    EnabledByPolicy,
}
pub trait Task {
    fn status(&self) -> Result<Status, String>;
    fn enable(&self) -> Result<Status, String>;
    fn disable(&self) -> Result<(), String>;
}
pub fn apply(task: &dyn Task, enabled: bool) -> Result<(), String> {
    use Status::*;
    let current = task.status()?;
    if enabled {
        match current {
            Enabled | EnabledByPolicy => Ok(()),
            DisabledByUser => Err("STARTUP_DISABLED_BY_USER".into()),
            DisabledByPolicy => Err("STARTUP_DISABLED_BY_POLICY".into()),
            Disabled => match task.enable()? {
                Enabled | EnabledByPolicy => Ok(()),
                DisabledByUser => Err("STARTUP_DISABLED_BY_USER".into()),
                DisabledByPolicy => Err("STARTUP_DISABLED_BY_POLICY".into()),
                Disabled => Err("STARTUP_CHANGE_FAILED".into()),
            },
        }
    } else {
        match current {
            Disabled | DisabledByUser | DisabledByPolicy => Ok(()),
            EnabledByPolicy => Err("STARTUP_DISABLED_BY_POLICY".into()),
            Enabled => {
                task.disable()?;
                match task.status()? {
                    Disabled | DisabledByUser | DisabledByPolicy => Ok(()),
                    _ => Err("STARTUP_CHANGE_FAILED".into()),
                }
            }
        }
    }
}

pub fn set(enabled: bool) -> Result<(), String> {
    #[cfg(windows)]
    {
        // WinRT activation and async completion stay in a dedicated MTA, away from WebView COM.
        std::thread::Builder::new()
            .name("prism-startup-task".into())
            .spawn(move || {
                use windows::{
                    ApplicationModel::{StartupTask, StartupTaskState},
                    Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED},
                };
                // SAFETY: this new worker owns one successful WinRT initialization/uninitialization.
                unsafe { RoInitialize(RO_INIT_MULTITHREADED) }
                    .map_err(|_| "STARTUP_UNAVAILABLE".to_string())?;
                struct Apartment;
                impl Drop for Apartment {
                    fn drop(&mut self) {
                        unsafe {
                            RoUninitialize();
                        }
                    }
                }
                let _apartment = Apartment;
                struct Native(StartupTask);
                fn convert(value: StartupTaskState) -> Result<Status, String> {
                    match value {
                        StartupTaskState::Disabled => Ok(Status::Disabled),
                        StartupTaskState::DisabledByUser => Ok(Status::DisabledByUser),
                        StartupTaskState::Enabled => Ok(Status::Enabled),
                        StartupTaskState::DisabledByPolicy => Ok(Status::DisabledByPolicy),
                        StartupTaskState::EnabledByPolicy => Ok(Status::EnabledByPolicy),
                        _ => Err("STARTUP_UNAVAILABLE".into()),
                    }
                }
                impl Task for Native {
                    fn status(&self) -> Result<Status, String> {
                        convert(self.0.State().map_err(|_| "STARTUP_UNAVAILABLE")?)
                    }
                    fn enable(&self) -> Result<Status, String> {
                        let state = self
                            .0
                            .RequestEnableAsync()
                            .and_then(|operation| operation.join())
                            .map_err(|_| "STARTUP_CHANGE_FAILED")?;
                        convert(state)
                    }
                    fn disable(&self) -> Result<(), String> {
                        self.0.Disable().map_err(|_| "STARTUP_CHANGE_FAILED".into())
                    }
                }
                let task = StartupTask::GetAsync(&TASK_ID.into())
                    .and_then(|operation| operation.join())
                    .map_err(|_| "STARTUP_UNAVAILABLE")?;
                apply(&Native(task), enabled)
            })
            .map_err(|_| "STARTUP_UNAVAILABLE")?
            .join()
            .map_err(|_| "STARTUP_UNAVAILABLE")?
    }
    #[cfg(not(windows))]
    {
        let _ = enabled;
        Err("STARTUP_UNAVAILABLE".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    struct Fake {
        state: Cell<Status>,
        enables: Cell<u32>,
        disables: Cell<u32>,
        result: Status,
    }
    impl Task for Fake {
        fn status(&self) -> Result<Status, String> {
            Ok(self.state.get())
        }
        fn enable(&self) -> Result<Status, String> {
            self.enables.set(self.enables.get() + 1);
            self.state.set(self.result);
            Ok(self.result)
        }
        fn disable(&self) -> Result<(), String> {
            self.disables.set(self.disables.get() + 1);
            self.state.set(Status::Disabled);
            Ok(())
        }
    }
    fn fake(state: Status, result: Status) -> Fake {
        Fake {
            state: Cell::new(state),
            result,
            enables: Cell::new(0),
            disables: Cell::new(0),
        }
    }
    #[test]
    fn user_and_policy_overrides_are_respected_without_a_request() {
        for state in [Status::DisabledByUser, Status::DisabledByPolicy] {
            let task = fake(state, Status::Enabled);
            assert!(apply(&task, true).is_err());
            assert_eq!(task.enables.get(), 0);
        }
        let task = fake(Status::EnabledByPolicy, Status::Enabled);
        assert!(apply(&task, false).is_err());
        assert_eq!(task.disables.get(), 0);
    }
    #[test]
    fn requested_changes_are_verified_and_existing_states_are_idempotent() {
        let task = fake(Status::Disabled, Status::Enabled);
        apply(&task, true).unwrap();
        apply(&task, true).unwrap();
        assert_eq!(task.enables.get(), 1);
        apply(&task, false).unwrap();
        apply(&task, false).unwrap();
        assert_eq!(task.disables.get(), 1);
        let refused = fake(Status::Disabled, Status::DisabledByUser);
        assert_eq!(
            apply(&refused, true).unwrap_err(),
            "STARTUP_DISABLED_BY_USER"
        );
    }
}
