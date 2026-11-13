//! Thin IPC boundary. Disk work never runs on the webview event thread.
use crate::{
    clipboard_history::{ClipItem, ClipKind, ClipListResponse, ClipStatus},
    AppState,
};
use tauri::{Manager, State, WebviewWindow};

#[tauri::command]
pub async fn clipboard_list(
    state: State<'_, AppState>,
    query: String,
    kind: Option<ClipKind>,
    limit: usize,
) -> Result<ClipListResponse, String> {
    let service = state.clipboard.clone();
    tauri::async_runtime::spawn_blocking(move || service.list(&query, kind, limit))
        .await
        .map_err(|_| "Clipboard request failed.")?
}
#[tauri::command]
pub async fn clipboard_status(state: State<'_, AppState>) -> Result<ClipStatus, String> {
    let service = state.clipboard.clone();
    tauri::async_runtime::spawn_blocking(move || service.status())
        .await
        .map_err(|_| "Clipboard request failed.")?
}
#[tauri::command]
pub async fn clipboard_text(state: State<'_, AppState>, id: String) -> Result<String, String> {
    let service = state.clipboard.clone();
    tauri::async_runtime::spawn_blocking(move || service.text(&id))
        .await
        .map_err(|_| "Clipboard request failed.")?
}
#[tauri::command]
pub async fn clipboard_thumbnail(state: State<'_, AppState>, id: String) -> Result<String, String> {
    let service = state.clipboard.clone();
    tauri::async_runtime::spawn_blocking(move || service.thumbnail_data_url(&id))
        .await
        .map_err(|_| "Clipboard request failed.")?
}
#[tauri::command]
pub async fn clipboard_copy(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let service = state.clipboard.clone();
    tauri::async_runtime::spawn_blocking(move || service.copy_to_clipboard(&id))
        .await
        .map_err(|_| "Clipboard request failed.")?
}
#[tauri::command]
pub async fn clipboard_paste(
    state: State<'_, AppState>,
    window: WebviewWindow,
    id: String,
) -> Result<crate::paste_target::PasteOutcome, String> {
    let service = state.clipboard.clone();
    let targets = state.paste_targets.clone();
    let backend = state.paste_input.clone();
    let app = window.app_handle().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut hidden = Vec::new();
        let outcome = targets.paste_with(
            backend.as_ref(),
            || service.copy_to_clipboard(&id),
            || {
                for label in ["quick", "main"] {
                    if let Some(window) = app.get_webview_window(label) {
                        if window.is_visible().map_err(|_| "Window unavailable.")? {
                            window.hide().map_err(|_| "Could not hide Prism.")?;
                            hidden.push(window);
                        }
                    }
                }
                Ok(())
            },
        );
        if !outcome.pasted && !hidden.is_empty() {
            for hidden in hidden {
                let _ = hidden.show();
            }
            // Do not recapture the target during failure recovery.
            let _ = window.set_focus();
        }
        outcome
    })
    .await
    .map_err(|_| "Clipboard paste request failed.".into())
}

/// Inert in release builds and ordinary installed debug profiles.
#[tauri::command]
pub fn paste_fixture(
    state: State<'_, AppState>,
    mode: Option<String>,
) -> Result<serde_json::Value, String> {
    #[cfg(debug_assertions)]
    if let (Some(input), Some(clipboard)) = (&state.input_fixture, &state.clipboard_fixture) {
        if let Some(mode) = mode {
            input.configure(&mode)?;
            state.paste_targets.clear();
            state.paste_targets.capture(input.as_ref());
            input.arm();
        }
        let mut snapshot = input.snapshot();
        snapshot["clipboard"] = match clipboard.content() {
            Some(crate::clipboard_history::ClipContent::Text(text)) => {
                serde_json::json!({"kind":"text","text":text})
            }
            Some(crate::clipboard_history::ClipContent::Image { width, height, .. }) => {
                serde_json::json!({"kind":"image","width":width,"height":height})
            }
            None => serde_json::Value::Null,
        };
        return Ok(snapshot);
    }
    let _ = (state, mode);
    Err("Synthetic paste fixtures require an isolated debug profile.".into())
}

/// Generate bounded synthetic history data through the real blob/index writer.
/// Ordinary profiles and release builds cannot use this fixture.
#[tauri::command]
pub async fn clipboard_capture_fixture(
    state: State<'_, AppState>,
    kind: String,
    nonce: String,
) -> Result<ClipStatus, String> {
    #[cfg(debug_assertions)]
    if state.input_fixture.is_some() && state.clipboard_fixture.is_some() {
        if nonce.len() != 32 || !nonce.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("Invalid synthetic clipboard nonce.".into());
        }
        let content = match kind.as_str() {
            "text" => crate::clipboard_history::ClipContent::Text(format!(
                "Prism 저장 회귀 {nonce} 한글 🙂"
            )),
            "image" => {
                let (width, height) = (96u32, 64u32);
                let mut rgba = Vec::with_capacity((width * height * 4) as usize);
                for y in 0..height {
                    for x in 0..width {
                        rgba.extend_from_slice(&[
                            26 + x as u8,
                            94 + y as u8,
                            67 + (x / 2) as u8,
                            255,
                        ]);
                    }
                }
                crate::clipboard_history::ClipContent::Image {
                    width,
                    height,
                    rgba,
                }
            }
            _ => return Err("Unknown synthetic clipboard kind.".into()),
        };
        let service = state.clipboard.clone();
        return tauri::async_runtime::spawn_blocking(move || {
            service.capture(content)?;
            service.status()
        })
        .await
        .map_err(|_| "Synthetic clipboard capture failed.".to_string())?;
    }
    let _ = (state, kind, nonce);
    Err("Synthetic clipboard capture requires an isolated debug profile.".into())
}
#[tauri::command]
pub async fn clipboard_pin(
    state: State<'_, AppState>,
    id: String,
    pinned: bool,
) -> Result<ClipItem, String> {
    let service = state.clipboard.clone();
    tauri::async_runtime::spawn_blocking(move || service.set_pinned(&id, pinned))
        .await
        .map_err(|_| "Clipboard request failed.")?
}
#[tauri::command]
pub async fn clipboard_delete(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let service = state.clipboard.clone();
    tauri::async_runtime::spawn_blocking(move || service.delete(&id))
        .await
        .map_err(|_| "Clipboard request failed.")?
}
#[tauri::command]
pub async fn clipboard_clear(
    state: State<'_, AppState>,
    include_pinned: bool,
) -> Result<(), String> {
    let service = state.clipboard.clone();
    tauri::async_runtime::spawn_blocking(move || service.clear(include_pinned))
        .await
        .map_err(|_| "Clipboard request failed.")?
}
#[tauri::command]
pub async fn clipboard_set_paused(
    state: State<'_, AppState>,
    paused: bool,
) -> Result<ClipStatus, String> {
    let service = state.clipboard.clone();
    tauri::async_runtime::spawn_blocking(move || service.set_paused(paused))
        .await
        .map_err(|_| "Clipboard request failed.")?
}
