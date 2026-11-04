//! Debug isolation only. No native windows, keyboard or system clipboard access.
use crate::paste_target::{InputBackend, WindowId};
use serde_json::{json, Value};
use std::{sync::Mutex, time::Duration};

#[derive(Default)]
struct State {
    mode: String,
    foreground: Option<WindowId>,
    pid: u32,
    focus: u32,
    attempts: u32,
    pasted: u32,
}
#[derive(Default)]
pub struct MemoryInput(Mutex<State>);
impl MemoryInput {
    pub fn configure(&self, mode: &str) -> Result<(), String> {
        if ![
            "ready",
            "noTarget",
            "targetGone",
            "focusFailed",
            "keysHeld",
            "inputBlocked",
            "unsupported",
        ]
        .contains(&mode)
        {
            return Err("Unknown synthetic paste scenario.".into());
        }
        *self.0.lock().map_err(|_| "Fixture unavailable.")? = State {
            mode: mode.into(),
            foreground: (mode != "noTarget").then_some(1),
            pid: 7,
            ..State::default()
        };
        Ok(())
    }
    pub fn arm(&self) {
        let mut state = self.0.lock().unwrap();
        state.foreground = Some(2);
        if state.mode == "targetGone" {
            state.pid = 8;
        }
    }
    pub fn snapshot(&self) -> Value {
        let state = self.0.lock().unwrap();
        json!({"mode":state.mode,"focusRequests":state.focus,"inputAttempts":state.attempts,"pasteRequests":state.pasted})
    }
}
impl InputBackend for MemoryInput {
    fn supported(&self) -> bool {
        self.0.lock().unwrap().mode != "unsupported"
    }
    fn current_pid(&self) -> u32 {
        100
    }
    fn foreground(&self) -> Option<WindowId> {
        self.0.lock().unwrap().foreground
    }
    fn window_pid(&self, window: WindowId) -> Option<u32> {
        match window {
            1 => Some(self.0.lock().unwrap().pid),
            2 => Some(100),
            _ => None,
        }
    }
    fn focus(&self, window: WindowId) -> bool {
        let mut state = self.0.lock().unwrap();
        state.focus += 1;
        if state.mode == "focusFailed" {
            return false;
        }
        state.foreground = Some(window);
        true
    }
    fn keys_released(&self) -> bool {
        self.0.lock().unwrap().mode != "keysHeld"
    }
    fn send_ctrl_v(&self, window: WindowId, pid: u32) -> bool {
        let mut state = self.0.lock().unwrap();
        state.attempts += 1;
        if state.mode == "inputBlocked" || state.foreground != Some(window) || state.pid != pid {
            return false;
        }
        state.pasted += 1;
        true
    }
    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}
