//! Remember opaque external foreground handles, never titles, paths or keystrokes.
//! Installed only on the native event-loop thread, never in isolated profiles.
use crate::paste_target::PasteTargets;
use std::sync::Arc;

#[cfg(windows)]
mod native {
    use super::*;
    use std::{cell::RefCell, sync::Weak};
    use windows_sys::Win32::{
        Foundation::HWND,
        UI::{
            Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK},
            WindowsAndMessaging::{
                GetWindowThreadProcessId, EVENT_SYSTEM_FOREGROUND, WINEVENT_OUTOFCONTEXT,
                WINEVENT_SKIPOWNPROCESS,
            },
        },
    };
    struct Tracker {
        hook: HWINEVENTHOOK,
        targets: Weak<PasteTargets>,
    }
    thread_local! { static TRACKER: RefCell<Option<Tracker>> = const { RefCell::new(None) }; }

    pub fn start(targets: &Arc<PasteTargets>) -> Result<(), String> {
        stop();
        // SAFETY: static callback, out-of-context on this Tauri event-loop thread. The
        // callback borrows only a Weak target; shutdown unregisters on this same thread.
        let hook = unsafe {
            SetWinEventHook(
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_FOREGROUND,
                std::ptr::null_mut(),
                Some(on_foreground),
                0,
                0,
                WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
            )
        };
        if hook.is_null() {
            return Err(
                "Automatic paste window tracking is unavailable. Clipboard copy remains available."
                    .into(),
            );
        }
        TRACKER.with(|slot| {
            *slot.borrow_mut() = Some(Tracker {
                hook,
                targets: Arc::downgrade(targets),
            })
        });
        Ok(())
    }
    pub fn stop() {
        let tracker = TRACKER.with(|slot| slot.borrow_mut().take());
        if let Some(tracker) = tracker {
            // SAFETY: called on the registering event-loop thread; no borrow is held while
            // unhooking. Windows also removes the hook when that thread exits.
            unsafe {
                UnhookWinEvent(tracker.hook);
            }
        }
    }
    unsafe extern "system" fn on_foreground(
        _: HWINEVENTHOOK,
        event: u32,
        window: HWND,
        _: i32,
        _: i32,
        _: u32,
        event_tick: u32,
    ) {
        if event != EVENT_SYSTEM_FOREGROUND || window.is_null() {
            return;
        }
        // Avoid panics across FFI and release the thread-local borrow before any OS call.
        let targets = TRACKER
            .try_with(|slot| {
                slot.try_borrow()
                    .ok()
                    .and_then(|slot| slot.as_ref().map(|tracker| tracker.targets.clone()))
            })
            .ok()
            .flatten()
            .and_then(|weak| weak.upgrade());
        if let Some(targets) = targets {
            let mut pid = 0;
            // SAFETY: hwnd is an opaque event handle; this query returns zero for a dead window.
            unsafe {
                GetWindowThreadProcessId(window, &mut pid);
            }
            // Event timestamps share GetTickCount's wrapping clock. Drop old queued events
            // instead of allowing them to overwrite a newer explicit shortcut capture.
            let now = unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount() };
            targets.remember_event(window as isize, pid, std::process::id(), event_tick, now);
        }
    }
}
pub fn start(targets: &Arc<PasteTargets>) -> Result<(), String> {
    #[cfg(windows)]
    {
        native::start(targets)
    }
    #[cfg(not(windows))]
    {
        let _ = targets;
        Ok(())
    }
}
pub fn stop() {
    #[cfg(windows)]
    native::stop();
}
