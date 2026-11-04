//! Thin IPC boundary. Disk work never runs on the webview event thread.
use crate::{
    clipboard_history::{ClipItem, ClipKind, ClipListResponse, ClipStatus},
    AppState,
};
use tauri::State;

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
pub fn clipboard_status(state: State<'_, AppState>) -> Result<ClipStatus, String> {
    state.clipboard.status()
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
pub fn clipboard_set_paused(
    state: State<'_, AppState>,
    paused: bool,
) -> Result<ClipStatus, String> {
    state.clipboard.set_paused(paused)
}
