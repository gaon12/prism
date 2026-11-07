//! Paste into the app that was in front before Prism opened.
//!
//! `capture` remembers the foreground window right before a Prism window is shown. `paste`
//! copies first, then restores that window and sends Ctrl+V only after the window is confirmed
//! to be in front. Copy and paste outcomes are reported separately so a failed paste never loses
//! the copy. No window titles are collected and nothing about the target is logged.

use serde::Serialize;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::{Duration, Instant},
};

/// A remembered target expires so an old window is never pasted into by surprise.
const TARGET_TTL: Duration = Duration::from_secs(300);
const FOCUS_TIMEOUT: Duration = Duration::from_millis(300);
const FOCUS_POLL: Duration = Duration::from_millis(15);
const KEY_RELEASE_TIMEOUT: Duration = Duration::from_millis(800);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PasteError {
    Busy,
    KeysHeld,
    /// No other app was in front when Prism opened.
    NoTarget,
    /// The window closed (or its handle now belongs to another process).
    TargetGone,
    /// Windows refused to bring the window to the front.
    FocusFailed,
    /// The complete input batch could not be confirmed; partial input is possible.
    InputBlocked,
    /// The copy itself failed; nothing was pasted.
    CopyFailed,
    /// Paste into other apps is not available on this platform.
    Unsupported,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PasteOutcome {
    pub copied: bool,
    /// Complete chord delivery, not acknowledgment by the receiving application.
    pub pasted: bool,
    pub error: Option<PasteError>,
}

/// Opaque native window handle (HWND on Windows).
pub type WindowId = isize;

/// OS access, injected so tests never touch the real clipboard, windows or keyboard.
pub trait InputBackend {
    fn supported(&self) -> bool;
    /// Reject pending input when the application is definitely exiting.
    fn shutdown(&self) {}
    fn current_pid(&self) -> u32;
    fn event_tick(&self) -> Option<u32> {
        None
    }
    fn foreground(&self) -> Option<WindowId>;
    /// Owning process id, or None when the handle is no longer a window.
    fn window_pid(&self, window: WindowId) -> Option<u32>;
    /// Restores a minimized window and asks for foreground. Returns the OS call's result.
    fn focus(&self, window: WindowId) -> bool;
    /// Never change physical key state to make the shortcut work.
    fn keys_released(&self) -> bool {
        true
    }
    /// Recheck the target immediately before injecting the complete chord.
    fn send_ctrl_v(&self, window: WindowId, pid: u32) -> bool;
    fn sleep(&self, duration: Duration);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PasteTarget {
    window: WindowId,
    pid: u32,
    captured: Instant,
    native_tick: Option<u32>,
}

#[derive(Default)]
pub struct PasteTargets {
    target: Mutex<Option<PasteTarget>>,
    active: Mutex<()>,
    disabled: AtomicBool,
}

impl PasteTargets {
    /// Call right before showing a Prism window. Prism's own windows are never remembered;
    /// in that case the previous target is kept (e.g. reopening from the main window).
    pub fn capture(&self, backend: &dyn InputBackend) -> bool {
        if self.disabled.load(Ordering::Acquire) || !backend.supported() {
            return false;
        }
        let Some(window) = backend.foreground() else {
            return false;
        };
        let Some(pid) = backend.window_pid(window) else {
            return false;
        };
        self.remember_at(
            window,
            pid,
            backend.current_pid(),
            backend.event_tick(),
            Duration::ZERO,
        )
    }
    /// Used by foreground events as well as explicit window-opening commands.
    #[cfg(test)]
    pub(crate) fn remember_external(&self, window: WindowId, pid: u32, own_pid: u32) -> bool {
        self.remember_at(window, pid, own_pid, None, Duration::ZERO)
    }
    pub(crate) fn remember_event(
        &self,
        window: WindowId,
        pid: u32,
        own_pid: u32,
        tick: u32,
        now: u32,
    ) -> bool {
        self.remember_at(
            window,
            pid,
            own_pid,
            Some(tick),
            Duration::from_millis(now.wrapping_sub(tick) as u64),
        )
    }
    fn remember_at(
        &self,
        window: WindowId,
        pid: u32,
        own_pid: u32,
        tick: Option<u32>,
        age: Duration,
    ) -> bool {
        if self.disabled.load(Ordering::Acquire) || window == 0 || pid == 0 || pid == own_pid {
            return false;
        }
        if age > TARGET_TTL {
            return false;
        }
        if let Ok(mut slot) = self.target.lock() {
            if let Some(previous) = slot
                .as_ref()
                .filter(|target| target.captured.elapsed() < TARGET_TTL)
            {
                if let (Some(previous_tick), Some(tick)) = (previous.native_tick, tick) {
                    let order = tick.wrapping_sub(previous_tick) as i32;
                    if order <= 0 {
                        // A queued event must not replace a newer explicit foreground capture.
                        // Conflicting handles within one clock tick are ambiguous: keep copy only.
                        if order == 0 && (window != previous.window || pid != previous.pid) {
                            *slot = None;
                        }
                        return false;
                    }
                }
            }
            *slot = Some(PasteTarget {
                window,
                pid,
                captured: Instant::now().checked_sub(age).unwrap_or_else(Instant::now),
                native_tick: tick,
            });
        }
        true
    }
    pub fn disable(&self) {
        self.disabled.store(true, Ordering::Release);
        self.clear();
    }
    pub fn clear(&self) {
        if let Ok(mut slot) = self.target.lock() {
            *slot = None;
        }
    }
    #[cfg(test)]
    pub fn has_target(&self) -> bool {
        self.valid_target(None).is_some()
    }
    fn valid_target(&self, backend: Option<&dyn InputBackend>) -> Option<PasteTarget> {
        let target = (*self.target.lock().ok()?)?;
        if target.captured.elapsed() > TARGET_TTL {
            return None;
        }
        match backend {
            Some(backend) => {
                (backend.window_pid(target.window) == Some(target.pid)).then_some(target)
            }
            None => Some(target),
        }
    }

    /// Convenience for isolated service tests without any window preparation.
    #[cfg(test)]
    pub fn paste(
        &self,
        backend: &dyn InputBackend,
        copy: impl FnOnce() -> Result<(), String>,
    ) -> PasteOutcome {
        self.paste_with(backend, copy, || Ok(()))
    }
    /// Prepare (hide Prism) only after copying and validating the remembered target.
    pub fn paste_with(
        &self,
        backend: &dyn InputBackend,
        copy: impl FnOnce() -> Result<(), String>,
        prepare: impl FnOnce() -> Result<(), String>,
    ) -> PasteOutcome {
        let fail = |copied, error| PasteOutcome {
            copied,
            pasted: false,
            error: Some(error),
        };
        let Ok(_operation) = self.active.try_lock() else {
            return fail(false, PasteError::Busy);
        };
        if copy().is_err() {
            return fail(false, PasteError::CopyFailed);
        }
        if self.disabled.load(Ordering::Acquire) {
            return fail(true, PasteError::NoTarget);
        }
        if !backend.supported() {
            return fail(true, PasteError::Unsupported);
        }
        let Some(target) = self.valid_target(None) else {
            return fail(true, PasteError::NoTarget);
        };
        if backend.window_pid(target.window) != Some(target.pid) {
            self.clear();
            return fail(true, PasteError::TargetGone);
        }
        let start = Instant::now();
        while !backend.keys_released() {
            if start.elapsed() >= KEY_RELEASE_TIMEOUT {
                return fail(true, PasteError::KeysHeld);
            }
            backend.sleep(FOCUS_POLL);
        }
        if self.disabled.load(Ordering::Acquire) {
            return fail(true, PasteError::NoTarget);
        }
        if backend.window_pid(target.window) != Some(target.pid) {
            self.clear();
            return fail(true, PasteError::TargetGone);
        }
        if prepare().is_err() {
            return fail(true, PasteError::FocusFailed);
        }
        if self.disabled.load(Ordering::Acquire) {
            return fail(true, PasteError::NoTarget);
        }
        if !self.bring_to_front(backend, target.window) {
            return fail(true, PasteError::FocusFailed);
        }
        if backend.window_pid(target.window) != Some(target.pid) {
            self.clear();
            return fail(true, PasteError::TargetGone);
        }
        if backend.foreground() != Some(target.window) {
            return fail(true, PasteError::FocusFailed);
        }
        if self.disabled.load(Ordering::Acquire) {
            return fail(true, PasteError::NoTarget);
        }
        if !backend.send_ctrl_v(target.window, target.pid) {
            return fail(true, PasteError::InputBlocked);
        }
        PasteOutcome {
            copied: true,
            pasted: true,
            error: None,
        }
    }
    /// Asks for foreground (one retry) and waits until the OS confirms it.
    fn bring_to_front(&self, backend: &dyn InputBackend, window: WindowId) -> bool {
        for _ in 0..2 {
            backend.focus(window);
            let start = Instant::now();
            loop {
                if backend.foreground() == Some(window) {
                    return true;
                }
                if start.elapsed() >= FOCUS_TIMEOUT {
                    break;
                }
                backend.sleep(FOCUS_POLL);
            }
        }
        false
    }
}

pub fn system_backend() -> Box<dyn InputBackend + Send + Sync> {
    Box::new(SystemBackend)
}

struct SystemBackend;

#[cfg(windows)]
impl InputBackend for SystemBackend {
    fn supported(&self) -> bool {
        true
    }
    fn current_pid(&self) -> u32 {
        std::process::id()
    }
    fn event_tick(&self) -> Option<u32> {
        // SAFETY: read-only monotonic Windows event-clock query.
        Some(unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount() })
    }
    fn foreground(&self) -> Option<WindowId> {
        // SAFETY: no arguments; returns a handle or null.
        let window = unsafe { win::GetForegroundWindow() };
        (!window.is_null()).then_some(window as WindowId)
    }
    fn window_pid(&self, window: WindowId) -> Option<u32> {
        let hwnd = window as win::HWND;
        let mut pid = 0u32;
        // SAFETY: IsWindow validates the handle; the pid out-pointer is a live local.
        unsafe {
            if win::IsWindow(hwnd) == 0 {
                return None;
            }
            win::GetWindowThreadProcessId(hwnd, &mut pid);
        }
        (pid != 0).then_some(pid)
    }
    fn focus(&self, window: WindowId) -> bool {
        let hwnd = window as win::HWND;
        // SAFETY: window-manager calls on a handle validated by the caller. Respect normal
        // foreground restrictions; do not synthesize Alt or alter foreground-lock settings.
        unsafe {
            if win::IsIconic(hwnd) != 0 {
                win::ShowWindow(hwnd, win::SW_RESTORE);
            }
            win::SetForegroundWindow(hwnd) != 0
        }
    }
    fn keys_released(&self) -> bool {
        [
            win::VK_CONTROL,
            win::VK_SHIFT,
            win::VK_MENU,
            win::VK_LWIN,
            win::VK_RWIN,
            0x0d,
            VK_V,
        ]
        .iter()
        .all(|key| {
            // SAFETY: reads key state without changing it.
            unsafe { win::GetAsyncKeyState(*key as i32) >= 0 }
        })
    }
    fn send_ctrl_v(&self, window: WindowId, pid: u32) -> bool {
        if !self.keys_released()
            || self.window_pid(window) != Some(pid)
            || self.foreground() != Some(window)
        {
            return false;
        }
        deliver_keys(send_keys)
    }
    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

// Keep the input plan independently testable, including partial SendInput delivery.
// Never retry the chord: a partial batch might already have initiated a paste.
#[cfg(any(windows, test))]
fn deliver_keys(mut send: impl FnMut(&[(u16, bool)]) -> usize) -> bool {
    const CTRL: u16 = 0x11;
    const V: u16 = 0x56;
    let keys = [(CTRL, false), (V, false), (V, true), (CTRL, true)];
    let count = send(&keys);
    if count == keys.len() {
        return true;
    }
    let cleanup: &[(u16, bool)] = match count {
        1 | 3 => &[(CTRL, true)],
        2 => &[(V, true), (CTRL, true)],
        _ => &[],
    };
    let mut remaining = cleanup;
    // Retry missing key-ups only, never repeat a key-down or a paste chord.
    for _ in 0..2 {
        if remaining.is_empty() {
            break;
        }
        let sent = send(remaining).min(remaining.len());
        remaining = &remaining[sent..];
    }
    false
}

#[cfg(windows)]
const VK_V: u16 = 0x56;

#[cfg(windows)]
fn send_keys(keys: &[(u16, bool)]) -> usize {
    let inputs: Vec<win::INPUT> = keys
        .iter()
        .map(|&(vk, up)| win::INPUT {
            r#type: win::INPUT_KEYBOARD,
            Anonymous: win::INPUT_0 {
                ki: win::KEYBDINPUT {
                    wVk: vk,
                    wScan: 0,
                    dwFlags: if up { win::KEYEVENTF_KEYUP } else { 0 },
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        })
        .collect();
    // SAFETY: the slice is valid for the call and cbSize matches INPUT.
    unsafe {
        win::SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            std::mem::size_of::<win::INPUT>() as i32,
        ) as usize
    }
}

#[cfg(windows)]
mod win {
    pub use windows_sys::Win32::Foundation::HWND;
    pub use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
        VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
    };
    pub use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowThreadProcessId, IsIconic, IsWindow, SetForegroundWindow,
        ShowWindow, SW_RESTORE,
    };
}

/// Native desktop adapters:
/// - macOS: `lsappinfo` reads the front app (no permission needed); `osascript` with System
///   Events activates it and presses Cmd+V. That needs the Accessibility permission for
///   Prism; without it the tool fails and the paste reports `InputBlocked` (copy is kept).
/// - Linux: native X11/XTest requests. Pure Wayland is reported as unsupported.
///
/// "Window" here is the app's process id on macOS and the X11 window id on Linux.
/// Every tool runs without a shell, with fixed arguments and a short timeout.
#[cfg(not(windows))]
impl InputBackend for SystemBackend {
    fn supported(&self) -> bool {
        unix_desktop::supported()
    }
    #[cfg(target_os = "linux")]
    fn shutdown(&self) {
        unix_desktop::shutdown();
    }
    fn current_pid(&self) -> u32 {
        std::process::id()
    }
    fn foreground(&self) -> Option<WindowId> {
        unix_desktop::foreground()
    }
    fn window_pid(&self, window: WindowId) -> Option<u32> {
        unix_desktop::window_pid(window)
    }
    fn focus(&self, window: WindowId) -> bool {
        unix_desktop::focus(window)
    }
    #[cfg(target_os = "linux")]
    fn keys_released(&self) -> bool {
        unix_desktop::keys_released()
    }
    fn send_ctrl_v(&self, window: WindowId, pid: u32) -> bool {
        #[cfg(target_os = "linux")]
        {
            unix_desktop::paste(window, pid)
        }
        #[cfg(not(target_os = "linux"))]
        {
            // Recheck right before the keystroke: the same target must still be in front.
            unix_desktop::foreground() == Some(window)
                && unix_desktop::window_pid(window) == Some(pid)
                && self.keys_released()
                && unix_desktop::paste()
        }
    }
    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

/// Runs a system tool with fixed arguments and returns its stdout when it exits successfully
/// within `timeout`. A tool that hangs is killed, so it can never freeze Prism.
#[cfg(target_os = "macos")]
fn run_tool(program: &str, args: &[&str], timeout: Duration) -> Option<String> {
    use std::process::{Command, Stdio};
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut text = String::new();
                if let Some(mut out) = child.stdout.take() {
                    use std::io::Read;
                    let _ = out.read_to_string(&mut text);
                }
                return status.success().then_some(text);
            }
            Ok(None) if start.elapsed() < timeout => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

#[cfg(target_os = "macos")]
mod unix_desktop {
    use super::{run_tool, WindowId};
    use std::time::Duration;

    const TOOL_TIMEOUT: Duration = Duration::from_secs(2);

    pub fn supported() -> bool {
        std::path::Path::new("/usr/bin/osascript").is_file()
            && std::path::Path::new("/usr/bin/lsappinfo").is_file()
    }
    pub fn foreground() -> Option<WindowId> {
        let front = run_tool("/usr/bin/lsappinfo", &["front"], TOOL_TIMEOUT)?;
        let front = front.trim();
        if front.is_empty() {
            return None;
        }
        let info = run_tool(
            "/usr/bin/lsappinfo",
            &["info", "-only", "pid", front],
            TOOL_TIMEOUT,
        )?;
        super::parse_lsappinfo_pid(&info).map(|pid| pid as WindowId)
    }
    pub fn window_pid(window: WindowId) -> Option<u32> {
        let pid = u32::try_from(window).ok().filter(|pid| *pid > 0)?;
        // kill(pid, 0) only checks that the process still exists; it sends nothing.
        (unsafe { libc::kill(pid as libc::pid_t, 0) } == 0).then_some(pid)
    }
    pub fn focus(window: WindowId) -> bool {
        let Some(pid) = window_pid(window) else {
            return false;
        };
        let script = format!(
            "tell application \"System Events\" to set frontmost of (first process whose unix id is {pid}) to true"
        );
        run_tool("/usr/bin/osascript", &["-e", &script], TOOL_TIMEOUT).is_some()
    }
    pub fn paste() -> bool {
        run_tool(
            "/usr/bin/osascript",
            &[
                "-e",
                "tell application \"System Events\" to keystroke \"v\" using command down",
            ],
            TOOL_TIMEOUT,
        )
        .is_some()
    }
}

#[cfg(target_os = "linux")]
#[path = "paste_x11.rs"]
mod native_x11;
#[cfg(target_os = "linux")]
use native_x11 as unix_desktop;

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
mod unix_desktop {
    use super::WindowId;
    pub fn supported() -> bool {
        false
    }
    pub fn foreground() -> Option<WindowId> {
        None
    }
    pub fn window_pid(_: WindowId) -> Option<u32> {
        None
    }
    pub fn focus(_: WindowId) -> bool {
        false
    }
    pub fn paste() -> bool {
        false
    }
}

/// `lsappinfo info -only pid ASN:...` prints `"pid"=1234`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn parse_lsappinfo_pid(text: &str) -> Option<u32> {
    let value = text.split('=').nth(1)?.trim();
    value.parse().ok().filter(|pid| *pid > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, collections::HashMap};

    const PRISM: u32 = 100;

    #[test]
    fn unix_tool_output_parsers() {
        // Tool output as printed, including the trailing newline.
        assert_eq!(parse_lsappinfo_pid("\"pid\"=4321\n"), Some(4321));
        assert_eq!(parse_lsappinfo_pid(""), None);
        assert_eq!(parse_lsappinfo_pid("\"pid\"=0"), None);
    }

    /// Fake window manager: no real windows, keyboard or clipboard are touched.
    #[derive(Default)]
    struct Fake {
        windows: RefCell<HashMap<WindowId, u32>>,
        foreground: RefCell<Option<WindowId>>,
        /// Focus requests that the OS refuses before one succeeds.
        refuse_focus: RefCell<u32>,
        /// Focus "succeeds" but foreground never actually changes.
        focus_lies: bool,
        input_blocked: bool,
        keys_held: bool,
        pid_after_focus: Option<u32>,
        log: RefCell<Vec<String>>,
    }
    impl Fake {
        fn with(windows: &[(WindowId, u32)], foreground: WindowId) -> Self {
            let fake = Self::default();
            fake.windows.borrow_mut().extend(windows.iter().copied());
            *fake.foreground.borrow_mut() = Some(foreground);
            fake
        }
        fn log(&self) -> Vec<String> {
            self.log.borrow().clone()
        }
    }
    impl InputBackend for Fake {
        fn supported(&self) -> bool {
            true
        }
        fn current_pid(&self) -> u32 {
            PRISM
        }
        fn foreground(&self) -> Option<WindowId> {
            *self.foreground.borrow()
        }
        fn window_pid(&self, window: WindowId) -> Option<u32> {
            self.windows.borrow().get(&window).copied()
        }
        fn focus(&self, window: WindowId) -> bool {
            self.log.borrow_mut().push(format!("focus {window}"));
            let mut refuse = self.refuse_focus.borrow_mut();
            if *refuse > 0 {
                *refuse -= 1;
                return false;
            }
            if !self.focus_lies {
                *self.foreground.borrow_mut() = Some(window);
            }
            if let Some(pid) = self.pid_after_focus {
                self.windows.borrow_mut().insert(window, pid);
            }
            true
        }
        fn keys_released(&self) -> bool {
            !self.keys_held
        }
        fn send_ctrl_v(&self, _: WindowId, _: u32) -> bool {
            self.log.borrow_mut().push("ctrl+v".into());
            !self.input_blocked
        }
        fn sleep(&self, _: Duration) {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    fn copy_ok() -> Result<(), String> {
        Ok(())
    }

    #[test]
    fn pastes_into_the_window_that_was_in_front() {
        let fake = Fake::with(&[(1, 7), (2, PRISM)], 1);
        let targets = PasteTargets::default();
        assert!(targets.capture(&fake));
        *fake.foreground.borrow_mut() = Some(2); // Prism is now in front.
        let outcome = targets.paste(&fake, copy_ok);
        assert_eq!(
            outcome,
            PasteOutcome {
                copied: true,
                pasted: true,
                error: None
            }
        );
        assert_eq!(fake.log(), ["focus 1", "ctrl+v"]);
        // The target stays for repeated pastes.
        assert!(targets.paste(&fake, copy_ok).pasted);
    }

    #[test]
    fn prism_windows_are_never_targets_and_keep_the_previous_target() {
        let fake = Fake::with(&[(1, 7), (2, PRISM)], 2);
        let targets = PasteTargets::default();
        assert!(!targets.capture(&fake));
        let outcome = targets.paste(&fake, copy_ok);
        assert_eq!(
            (outcome.copied, outcome.error),
            (true, Some(PasteError::NoTarget))
        );
        *fake.foreground.borrow_mut() = Some(1);
        assert!(targets.capture(&fake));
        *fake.foreground.borrow_mut() = Some(2);
        assert!(!targets.capture(&fake));
        assert!(targets.has_target());
        assert!(targets.paste(&fake, copy_ok).pasted);
    }

    #[test]
    fn closed_or_reused_windows_are_rejected_without_input() {
        let fake = Fake::with(&[(1, 7)], 1);
        let targets = PasteTargets::default();
        targets.capture(&fake);
        fake.windows.borrow_mut().insert(1, 8); // Handle reused by another process.
        let outcome = targets.paste(&fake, copy_ok);
        assert_eq!(
            (outcome.copied, outcome.error),
            (true, Some(PasteError::TargetGone))
        );
        assert!(!targets.has_target());
        targets.capture(&fake);
        fake.windows.borrow_mut().clear(); // Window closed.
        assert_eq!(
            targets.paste(&fake, copy_ok).error,
            Some(PasteError::TargetGone)
        );
        assert!(
            fake.log().is_empty(),
            "no focus or input for missing targets"
        );
    }

    #[test]
    fn copy_failure_stops_before_focus_or_input() {
        let fake = Fake::with(&[(1, 7)], 1);
        let targets = PasteTargets::default();
        targets.capture(&fake);
        let outcome = targets.paste(&fake, || Err("busy".into()));
        assert_eq!(
            outcome,
            PasteOutcome {
                copied: false,
                pasted: false,
                error: Some(PasteError::CopyFailed)
            }
        );
        assert!(fake.log().is_empty());
    }

    #[test]
    fn focus_is_retried_once_and_confirmed_before_ctrl_v() {
        let fake = Fake::with(&[(1, 7), (2, PRISM)], 1);
        let targets = PasteTargets::default();
        targets.capture(&fake);
        *fake.foreground.borrow_mut() = Some(2);
        *fake.refuse_focus.borrow_mut() = 1;
        assert!(targets.paste(&fake, copy_ok).pasted);
        assert_eq!(fake.log(), ["focus 1", "focus 1", "ctrl+v"]);

        // The OS claims success but the window never comes to the front: no Ctrl+V.
        let liar = Fake {
            focus_lies: true,
            ..Fake::with(&[(1, 7), (2, PRISM)], 1)
        };
        targets.capture(&liar);
        *liar.foreground.borrow_mut() = Some(2);
        let outcome = targets.paste(&liar, copy_ok);
        assert_eq!(
            (outcome.copied, outcome.error),
            (true, Some(PasteError::FocusFailed))
        );
        assert!(!liar.log().contains(&"ctrl+v".to_string()));
    }

    #[test]
    fn blocked_input_reports_copy_success_and_paste_failure() {
        let fake = Fake {
            input_blocked: true,
            ..Fake::with(&[(1, 7)], 1)
        };
        let targets = PasteTargets::default();
        targets.capture(&fake);
        let outcome = targets.paste(&fake, copy_ok);
        assert_eq!(
            outcome,
            PasteOutcome {
                copied: true,
                pasted: false,
                error: Some(PasteError::InputBlocked)
            }
        );
        let json = serde_json::to_string(&outcome).unwrap();
        assert_eq!(
            json,
            r#"{"copied":true,"pasted":false,"error":"inputBlocked"}"#
        );
    }

    #[test]
    fn expired_targets_are_not_used() {
        let fake = Fake::with(&[(1, 7)], 1);
        let targets = PasteTargets::default();
        targets.capture(&fake);
        if let Ok(mut slot) = targets.target.lock() {
            if let Some(target) = slot.as_mut() {
                target.captured = Instant::now() - TARGET_TTL - Duration::from_secs(1);
            }
        }
        assert_eq!(
            targets.paste(&fake, copy_ok).error,
            Some(PasteError::NoTarget)
        );
    }

    #[test]
    fn partial_chords_release_only_inserted_keys_and_are_never_retried() {
        for (delivered, cleanup) in [
            (0, vec![]),
            (1, vec![(0x11, true)]),
            (2, vec![(0x56, true), (0x11, true)]),
            (3, vec![(0x11, true)]),
        ] {
            let mut calls = vec![];
            assert!(!deliver_keys(|keys| {
                calls.push(keys.to_vec());
                if calls.len() == 1 {
                    delivered
                } else {
                    keys.len()
                }
            }));
            assert_eq!(
                calls[0],
                [(0x11, false), (0x56, false), (0x56, true), (0x11, true)]
            );
            if cleanup.is_empty() {
                assert_eq!(calls.len(), 1);
            } else {
                assert_eq!(calls, [calls[0].clone(), cleanup]);
            }
        }
        let mut calls = 0;
        assert!(deliver_keys(|keys| {
            calls += 1;
            keys.len()
        }));
        assert_eq!(calls, 1);
    }

    #[test]
    fn a_second_request_does_not_copy_or_prepare() {
        let fake = Fake::with(&[(1, 7)], 1);
        let targets = PasteTargets::default();
        let _guard = targets.active.lock().unwrap();
        let result = targets.paste_with(&fake, || panic!("copy"), || panic!("prepare"));
        assert_eq!(
            (result.copied, result.error),
            (false, Some(PasteError::Busy))
        );
        assert!(fake.log().is_empty());
    }

    #[test]
    fn partial_cleanup_only_retries_the_missing_release() {
        let mut calls = Vec::new();
        let mut counts = [2, 1, 1].into_iter();
        assert!(!deliver_keys(|keys| {
            calls.push(keys.to_vec());
            counts.next().expect("bounded cleanup")
        }));
        assert_eq!(calls[1], [(0x56, true), (0x11, true)]);
        assert_eq!(calls[2], [(0x11, true)]);
        let mut blocked = 0;
        assert!(!deliver_keys(|_| {
            blocked += 1;
            if blocked == 1 {
                2
            } else {
                0
            }
        }));
        assert_eq!(blocked, 3, "blocked cleanup is bounded");
    }

    #[test]
    fn held_shortcut_keys_preserve_copy_without_hiding_or_synthetic_key_ups() {
        let fake = Fake {
            keys_held: true,
            ..Fake::with(&[(1, 7)], 1)
        };
        let targets = PasteTargets::default();
        targets.capture(&fake);
        let result = targets.paste_with(&fake, copy_ok, || panic!("must remain visible"));
        assert_eq!(
            (result.copied, result.error),
            (true, Some(PasteError::KeysHeld))
        );
        assert!(fake.log().is_empty());
    }

    #[test]
    fn preparation_failure_preserves_copy_without_focus_or_input() {
        let fake = Fake::with(&[(1, 7)], 1);
        let targets = PasteTargets::default();
        targets.capture(&fake);
        let result = targets.paste_with(&fake, copy_ok, || Err("window unavailable".into()));
        assert_eq!(
            (result.copied, result.error),
            (true, Some(PasteError::FocusFailed))
        );
        assert!(fake.log().is_empty());
    }

    #[test]
    fn shutdown_during_preparation_never_focuses_or_sends_input() {
        let targets = PasteTargets::default();
        let fake = Fake::with(&[(10, 200)], 10);
        assert!(targets.capture(&fake));
        let result = targets.paste_with(&fake, copy_ok, || {
            targets.disable();
            Ok(())
        });
        assert_eq!(result.error, Some(PasteError::NoTarget));
        assert!(result.copied && !result.pasted);
        assert!(fake.log().is_empty());
    }

    #[test]
    fn target_identity_is_rechecked_after_focus() {
        let fake = Fake {
            pid_after_focus: Some(8),
            ..Fake::with(&[(1, 7)], 1)
        };
        let targets = PasteTargets::default();
        targets.capture(&fake);
        let result = targets.paste(&fake, copy_ok);
        assert_eq!(
            (result.copied, result.error),
            (true, Some(PasteError::TargetGone))
        );
        assert_eq!(fake.log(), ["focus 1"]);
    }

    #[test]
    fn foreground_events_keep_the_latest_external_app_and_ignore_prism() {
        let targets = PasteTargets::default();
        assert!(targets.remember_external(1, 7, PRISM));
        assert!(targets.remember_external(3, 9, PRISM));
        assert!(!targets.remember_external(2, PRISM, PRISM));
        assert!(!targets.remember_external(0, 7, PRISM));
        assert!(!targets.remember_external(1, 0, PRISM));
        let fake = Fake::with(&[(1, 7), (2, PRISM), (3, 9)], 2);
        assert!(targets.paste(&fake, copy_ok).pasted);
        assert_eq!(fake.log(), ["focus 3", "ctrl+v"]);
        targets.disable();
        assert!(!targets.capture(&fake));
        assert!(!targets.remember_external(1, 7, PRISM));
        assert_eq!(
            targets.paste(&fake, copy_ok).error,
            Some(PasteError::NoTarget)
        );
    }

    #[test]
    fn late_foreground_events_cannot_replace_newer_captures_and_ties_fail_closed() {
        let targets = PasteTargets::default();
        assert!(targets.remember_event(1, 7, PRISM, 100, 100));
        assert!(targets.remember_event(3, 9, PRISM, 200, 200));
        assert!(!targets.remember_event(1, 7, PRISM, 150, 210));
        let fake = Fake::with(&[(1, 7), (2, PRISM), (3, 9)], 2);
        assert!(targets.paste(&fake, copy_ok).pasted);
        assert_eq!(fake.log(), ["focus 3", "ctrl+v"]);
        assert!(!targets.remember_event(1, 7, PRISM, 200, 210));
        assert!(
            !targets.has_target(),
            "ambiguous same-tick changes never choose a target"
        );
        assert!(!targets.remember_event(1, 7, PRISM, 1, 300002));
        assert!(targets.remember_event(1, 7, PRISM, u32::MAX - 1, u32::MAX - 1));
        assert!(
            targets.remember_event(3, 9, PRISM, 3, 4),
            "event clock wrap stays ordered"
        );
    }
}
