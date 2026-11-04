//! Debug-isolated update transport. Never fetches or executes a package.
use crate::update_service::{AvailableUpdate, UpdateBackend};
use std::sync::Mutex;
#[derive(Default)]
struct State {
    mode: String,
    checks: u32,
    installs: u32,
}
#[derive(Default)]
pub struct MemoryBackend(Mutex<State>);
impl MemoryBackend {
    pub fn configure(&self, mode: &str) -> Result<(), String> {
        if ![
            "none",
            "ready",
            "newer",
            "offline",
            "signatureFailed",
            "installFailed",
            "newerInstallFailed",
        ]
        .contains(&mode)
        {
            return Err("Unknown update fixture.".into());
        }
        *self.0.lock().unwrap() = State {
            mode: mode.into(),
            ..Default::default()
        };
        Ok(())
    }
    pub fn snapshot(&self) -> serde_json::Value {
        let state = self.0.lock().unwrap();
        serde_json::json!({"mode":state.mode,"checks":state.checks,"installs":state.installs})
    }
}
impl UpdateBackend for MemoryBackend {
    fn check(&self) -> Result<Option<AvailableUpdate>, String> {
        let mut state = self.0.lock().unwrap();
        state.checks += 1;
        match state.mode.as_str() {
            "offline" => Err("The update server could not be reached.".into()),
            "ready" | "signatureFailed" | "installFailed" | "newer" | "newerInstallFailed" => {
                Ok(Some(AvailableUpdate {
                    version: if state.mode == "newer" || state.mode == "newerInstallFailed" {
                        "0.3.0"
                    } else {
                        "0.2.0"
                    }
                    .into(),
                    notes: "Synthetic release notes <script>plain text only</script>".into(),
                }))
            }
            _ => Ok(None),
        }
    }
    fn can_install(&self) -> bool {
        matches!(
            self.0.lock().unwrap().mode.as_str(),
            "signatureFailed" | "installFailed" | "newerInstallFailed"
        )
    }
    fn install(&self, _: &str, prepare: &dyn Fn() -> Result<(), String>) -> Result<(), String> {
        let mode = {
            let mut state = self.0.lock().unwrap();
            state.installs += 1;
            state.mode.clone()
        };
        if mode == "signatureFailed" {
            return Err("The update signature or signed version could not be verified.".into());
        }
        if mode != "installFailed" && mode != "newerInstallFailed" {
            return Err("Synthetic transport never installs packages.".into());
        }
        prepare()?;
        Err("The verified update could not be installed.".into())
    }
}
