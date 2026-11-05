use crate::{
    content_service::{ContentError, ContentStatus},
    content_store::{ContentChunk, ContentSearchResponse},
    vector_store::VectorSearchResponse,
    AppState,
};
use std::path::PathBuf;
use tauri::{AppHandle, State, WebviewWindow};
use tauri_plugin_opener::OpenerExt;
#[tauri::command]
pub async fn search_automatic(
    window: WebviewWindow,
    state: State<'_, AppState>,
    query: String,
    extension: String,
    scope_id: Option<String>,
    request_id: String,
) -> Result<crate::automatic_search::AutomaticResponse, ContentError> {
    let (semantic_enabled, hybrid) = {
        let settings = state.settings.lock().map_err(|_| worker_error())?;
        (settings.automatic_semantic, settings.hybrid_search)
    };
    let service = state.content.clone()?;
    let label = window.label().to_string();
    let cancel = service.begin_search(&label, &request_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        let result = service.automatic_search(
            &query,
            &extension,
            scope_id.as_deref(),
            semantic_enabled,
            hybrid,
            &cancel,
        );
        service.cancel_search(&label, &request_id);
        result
    })
    .await
    .map_err(|_| worker_error())?
}
#[tauri::command]
pub async fn search_hybrid(
    window: WebviewWindow,
    state: State<'_, AppState>,
    query: String,
    extension: String,
    scope_id: Option<String>,
    request_id: String,
) -> Result<crate::hybrid_search::HybridResponse, ContentError> {
    let service = state.content.clone()?;
    let label = window.label().to_string();
    let cancel = service.begin_search(&label, &request_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        let result = service.hybrid_search(&query, &extension, scope_id.as_deref(), &cancel);
        service.cancel_search(&label, &request_id);
        result
    })
    .await
    .map_err(|_| worker_error())?
}
fn worker_error() -> ContentError {
    ContentError {
        code: "worker".into(),
        message: "Content worker stopped unexpectedly.".into(),
    }
}
#[tauri::command]
pub fn content_status(state: State<'_, AppState>) -> Result<ContentStatus, ContentError> {
    state.content.as_ref().map_err(Clone::clone)?.status()
}
#[tauri::command]
pub fn content_refresh(state: State<'_, AppState>) -> Result<(), ContentError> {
    state.content.as_ref().map_err(Clone::clone)?.refresh()
}
#[tauri::command]
pub fn content_cancel_index(state: State<'_, AppState>) -> Result<(), ContentError> {
    state.content.as_ref().map_err(Clone::clone)?.cancel_index();
    Ok(())
}
#[tauri::command]
pub fn content_cancel_search(
    window: WebviewWindow,
    state: State<'_, AppState>,
    request_id: String,
) -> Result<(), ContentError> {
    state
        .content
        .as_ref()
        .map_err(Clone::clone)?
        .cancel_search(window.label(), &request_id);
    Ok(())
}
#[tauri::command]
pub async fn search_content(
    window: WebviewWindow,
    state: State<'_, AppState>,
    query: String,
    extension: String,
    scope_id: Option<String>,
    request_id: String,
) -> Result<ContentSearchResponse, ContentError> {
    let service = state.content.clone()?;
    let label = window.label().to_string();
    let cancel = service.begin_search(&label, &request_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        let result = service.search(&query, &extension, scope_id.as_deref(), &cancel);
        service.cancel_search(&label, &request_id);
        result
    })
    .await
    .map_err(|_| worker_error())?
}
#[tauri::command]
pub async fn search_semantic(
    window: WebviewWindow,
    state: State<'_, AppState>,
    query: String,
    extension: String,
    scope_id: Option<String>,
    request_id: String,
) -> Result<VectorSearchResponse, ContentError> {
    let service = state.content.clone()?;
    let label = window.label().to_string();
    let cancel = service.begin_search(&label, &request_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        let result = service.semantic_search(&query, &extension, scope_id.as_deref(), &cancel);
        service.cancel_search(&label, &request_id);
        result
    })
    .await
    .map_err(|_| worker_error())?
}
#[tauri::command]
pub async fn content_chunks(
    state: State<'_, AppState>,
    ids: Vec<String>,
) -> Result<Vec<ContentChunk>, ContentError> {
    let service = state.content.clone()?;
    tauri::async_runtime::spawn_blocking(move || {
        service.chunks(&ids, &std::sync::atomic::AtomicBool::new(false))
    })
    .await
    .map_err(|_| worker_error())?
}
#[tauri::command]
pub async fn content_file_action(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
    reveal: bool,
) -> Result<(), String> {
    let service = state.content.clone().map_err(|error| error.message)?;
    tauri::async_runtime::spawn_blocking(move || {
        let path = service
            .resolve_document(&PathBuf::from(path))
            .map_err(|error| error.message)?;
        if reveal {
            app.opener().reveal_item_in_dir(path)
        } else {
            app.opener()
                .open_path(path.to_string_lossy().into_owned(), None::<&str>)
        }
        .map_err(|_| "Could not open the content source.".to_string())
    })
    .await
    .map_err(|_| "Content worker stopped.".to_string())?
}

#[tauri::command]
pub async fn content_unlock_pdf(
    window: WebviewWindow,
    state: State<'_, AppState>,
    path: String,
    password: String,
    request_id: String,
) -> Result<crate::content_store::ContentDocumentSummary, ContentError> {
    let password = zeroize::Zeroizing::new(password);
    let service = state.content.clone()?;
    let permit = service.begin_unlock(window.label(), &request_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        // Both the permit and the password drop even when the caller disappears
        // or parsing fails. The permit keeps cancelled parsing within its slot.
        service.unlock_pdf(&PathBuf::from(path), &password, permit.cancel())
    })
    .await
    .map_err(|_| worker_error())?
}
#[tauri::command]
pub fn content_cancel_unlock(
    window: WebviewWindow,
    state: State<'_, AppState>,
    request_id: String,
) -> Result<(), ContentError> {
    state
        .content
        .as_ref()
        .map_err(Clone::clone)?
        .cancel_unlock(window.label(), &request_id);
    Ok(())
}

#[tauri::command]
pub fn content_prepare_unlock(
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<String, ContentError> {
    state
        .content
        .as_ref()
        .map_err(Clone::clone)?
        .prepare_unlock(window.label())
}
