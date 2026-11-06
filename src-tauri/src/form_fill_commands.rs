//! Tauri commands for filling an existing DOCX/HWPX form (requirement Q46).
//!
//! - Only the main window may use them (the quick-search window has no form UI).
//! - One form operation runs at a time; a second request gets "busy" instead of queueing
//!   more large reads in the background.
//! - The work runs on a blocking thread with the 60 s deadline inside `form_fill`, so a large
//!   form never stalls the UI thread.
//!
//! Errors are short codes ("input", "unsupported", "damaged", "tooLarge", "invalid",
//! "changed", "timedOut", "busy", "io"); the frontend has a message for each. File contents
//! and values are never logged.
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};

use crate::form_fill;

/// Set while a form is being read or saved.
static BUSY: AtomicBool = AtomicBool::new(false);

/// Holds the "busy" flag for one operation and clears it when dropped, even after a panic.
struct Admission;
impl Admission {
    fn take() -> Result<Self, String> {
        BUSY.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| Admission)
            .map_err(|_| "busy".to_string())
    }
}
impl Drop for Admission {
    fn drop(&mut self) {
        BUSY.store(false, Ordering::Release);
    }
}

fn main_window(window: &tauri::Window) -> Result<(), String> {
    if window.label() == "main" {
        Ok(())
    } else {
        Err("input".into())
    }
}

/// Runs `work` on a blocking thread while holding the admission.
async fn run<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, &'static str> + Send + 'static,
) -> Result<T, String> {
    let admission = Admission::take()?;
    let result = tauri::async_runtime::spawn_blocking(move || {
        let _admission = admission;
        work().map_err(String::from)
    })
    .await
    .map_err(|_| "io".to_string())?;
    result
}

#[tauri::command]
pub async fn form_fields(
    window: tauri::Window,
    path: String,
) -> Result<form_fill::FormInfo, String> {
    main_window(&window)?;
    run(move || form_fill::read_form(&PathBuf::from(path))).await
}

#[tauri::command]
pub async fn form_fill(
    window: tauri::Window,
    source: String,
    destination: String,
    values: HashMap<String, String>,
    fingerprint: String,
) -> Result<(), String> {
    main_window(&window)?;
    run(move || {
        form_fill::save_filled(
            &PathBuf::from(source),
            &PathBuf::from(destination),
            &values,
            &fingerprint,
        )
    })
    .await
}
