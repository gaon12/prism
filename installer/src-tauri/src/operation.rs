//! Serialize native requests and bind launching to a successfully installed directory.
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

#[derive(Default)]
struct State {
    busy: bool,
    closing: bool,
    installed: Option<PathBuf>,
    /// True when `installed` is the earlier version kept after a failed update (see `fail`).
    /// Its launch must re-check the folder under the folder lock.
    fallback: bool,
}
#[derive(Default)]
pub struct Gate(Mutex<State>);
pub struct Running {
    gate: Arc<Gate>,
    completed: bool,
}
impl Gate {
    pub fn begin(self: &Arc<Self>) -> Result<Running, String> {
        let mut state = self.0.lock().map_err(|_| "Setup state is unavailable.")?;
        if state.busy || state.closing {
            return Err("Setup is already changing application files.".into());
        }
        state.busy = true;
        state.installed = None;
        state.fallback = false;
        Ok(Running {
            gate: self.clone(),
            completed: false,
        })
    }
    pub fn busy(&self) -> bool {
        self.0.lock().map(|state| state.busy).unwrap_or(true)
    }
    pub fn finish(&self, launch: bool) -> Result<Option<PathBuf>, String> {
        let mut state = self.0.lock().map_err(|_| "Setup state is unavailable.")?;
        if state.busy {
            return Err("Wait for setup to finish before closing.".into());
        }
        if state.closing {
            return Err("Setup is closing.".into());
        }
        let target = if launch {
            Some(
                state
                    .installed
                    .clone()
                    .ok_or("Install Prism successfully before launching it.")?,
            )
        } else {
            None
        };
        state.closing = true;
        Ok(target)
    }
    /// True when the launch target is an earlier version kept after a failed update.
    pub fn launch_is_fallback(&self) -> bool {
        self.0.lock().map(|state| state.fallback).unwrap_or(true)
    }
    pub fn cancel_finish(&self) {
        if let Ok(mut state) = self.0.lock() {
            state.closing = false;
        }
    }
}
impl Running {
    pub fn complete(mut self, installed: Option<PathBuf>) -> Result<(), String> {
        let mut state = self
            .gate
            .0
            .lock()
            .map_err(|_| "Setup state is unavailable.")?;
        state.installed = installed;
        state.fallback = false;
        state.busy = false;
        self.completed = true;
        Ok(())
    }
    /// Ends a failed request. `fallback` is the folder of an intact earlier installation that
    /// the updater asked to restart (`/R`); it may then be launched by `finish(true)`.
    /// `None` keeps launching refused, like any other failure.
    pub fn fail(mut self, fallback: Option<PathBuf>) {
        if let Ok(mut state) = self.gate.0.lock() {
            state.fallback = fallback.is_some();
            state.installed = fallback;
            state.busy = false;
        }
        self.completed = true;
    }
}
impl Drop for Running {
    fn drop(&mut self) {
        if !self.completed {
            if let Ok(mut state) = self.gate.0.lock() {
                state.busy = false;
                state.installed = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn active_work_blocks_another_request_launch_and_close() {
        let gate = Arc::new(Gate::default());
        let request = gate.begin().unwrap();
        assert!(gate.begin().is_err());
        assert!(gate.finish(false).is_err());
        assert!(gate.finish(true).is_err());
        drop(request);
        assert!(!gate.busy());
        assert!(gate.finish(true).is_err());
    }
    #[test]
    fn only_a_successful_install_authorizes_its_native_launch_target() {
        let gate = Arc::new(Gate::default());
        let approved = PathBuf::from("approved-installation");
        gate.begin()
            .unwrap()
            .complete(Some(approved.clone()))
            .unwrap();
        assert_eq!(gate.finish(true).unwrap(), Some(approved));
        assert!(
            gate.begin().is_err(),
            "finish must reserve closing before the app exits"
        );
        gate.cancel_finish();
        // Beginning another operation revokes the previous launch authorization.
        drop(gate.begin().unwrap());
        assert!(gate.finish(true).is_err());
        gate.begin().unwrap().complete(None).unwrap();
        assert_eq!(gate.finish(false).unwrap(), None);
        assert!(gate.finish(true).is_err());
    }
    #[test]
    fn a_failed_update_launches_only_an_approved_fallback() {
        let gate = Arc::new(Gate::default());
        // Plain failure: still nothing to launch.
        gate.begin().unwrap().fail(None);
        assert!(!gate.busy());
        assert!(gate.finish(true).is_err());
        assert!(!gate.launch_is_fallback());
        // Failure with an intact previous installation: that folder may be restarted,
        // and the caller is told to re-check it under the folder lock.
        let previous = PathBuf::from("previous-installation");
        gate.begin().unwrap().fail(Some(previous.clone()));
        assert!(gate.launch_is_fallback());
        assert_eq!(gate.finish(true).unwrap(), Some(previous));
        // A later successful install is a normal launch again.
        gate.cancel_finish();
        gate.begin()
            .unwrap()
            .complete(Some(PathBuf::from("new")))
            .unwrap();
        assert!(!gate.launch_is_fallback());
    }
}
