use crate::{rag_service::*, secret_store::SecretValue, AppState};
use tauri::{State, WebviewWindow};

fn service(state: &AppState) -> RagResult<RagService> {
    state.rag.clone()
}
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> RagResult<T> + Send + 'static,
) -> RagResult<T> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|_| RagError::new(RagErrorCode::Worker))?
}
#[tauri::command]
pub async fn rag_create(
    state: State<'_, AppState>,
    title: Option<String>,
) -> RagResult<ConversationSummary> {
    let service = service(&state)?;
    blocking(move || service.create_conversation(title.as_deref())).await
}
#[tauri::command]
pub async fn rag_create_filename_search(
    window: WebviewWindow,
    state: State<'_, AppState>,
    search: crate::index::FilenameSearch,
    request_id: String,
) -> RagResult<ConversationSummary> {
    let service = service(&state)?;
    let index = state.index.clone();
    let keyboard = {
        let settings = state
            .settings
            .lock()
            .map_err(|_| RagError::new(RagErrorCode::Worker))?;
        if settings.revision != search.settings_revision {
            return Err(RagError::new(RagErrorCode::PolicyChanged));
        }
        settings.keyboard_correction
    };
    let content = state
        .content
        .clone()
        .map_err(|_| RagError::new(RagErrorCode::Worker))?;
    let label = window.label().to_owned();
    let cancel = content
        .begin_search(&label, &request_id)
        .map_err(|_| RagError::new(RagErrorCode::Input))?;
    blocking(move || {
        let result = service.create_filename_search(&index, &search, keyboard, &cancel);
        content.cancel_search(&label, &request_id);
        result
    })
    .await
}
#[tauri::command]
pub async fn rag_create_keyword_search(
    window: WebviewWindow,
    state: State<'_, AppState>,
    search: crate::content_service::KeywordSearch,
    request_id: String,
) -> RagResult<ConversationSummary> {
    let service = service(&state)?;
    if state
        .settings
        .lock()
        .map_err(|_| RagError::new(RagErrorCode::Worker))?
        .revision
        != search.settings_revision
    {
        return Err(RagError::new(RagErrorCode::PolicyChanged));
    }
    let content = state
        .content
        .clone()
        .map_err(|_| RagError::new(RagErrorCode::Worker))?;
    let label = window.label().to_owned();
    let cancel = content
        .begin_search(&label, &request_id)
        .map_err(|_| RagError::new(RagErrorCode::Input))?;
    blocking(move || {
        let result = service.create_keyword_search(&content, &search, &cancel);
        content.cancel_search(&label, &request_id);
        result
    })
    .await
}
#[tauri::command]
pub async fn rag_file_set_page(
    state: State<'_, AppState>,
    id: String,
    offset: u64,
) -> RagResult<Option<crate::selection_set::SelectionPage>> {
    let service = service(&state)?;
    blocking(move || service.file_set_page(&id, offset)).await
}
#[tauri::command]
pub async fn rag_edit_file_set(
    state: State<'_, AppState>,
    id: String,
    revision: u64,
    changes: Vec<(u64, bool)>,
    all: Option<bool>,
    clear: bool,
) -> RagResult<()> {
    let service = service(&state)?;
    blocking(move || service.edit_file_set(&id, revision, &changes, all, clear)).await
}
#[tauri::command]
pub async fn rag_create_from_search(
    state: State<'_, AppState>,
    selection: crate::chat_selection::ChatSelection,
) -> RagResult<ConversationSummary> {
    let service = service(&state)?;
    blocking(move || service.create_from_search(selection)).await
}
#[tauri::command]
pub async fn rag_set_selection(
    state: State<'_, AppState>,
    id: String,
    selection: Option<crate::chat_selection::ChatSelection>,
) -> RagResult<()> {
    let service = service(&state)?;
    blocking(move || service.set_selection(&id, selection)).await
}
#[tauri::command]
pub async fn rag_list(
    state: State<'_, AppState>,
    query: String,
    limit: usize,
) -> RagResult<Vec<ConversationSummary>> {
    let service = service(&state)?;
    blocking(move || service.list_conversations(&query, limit)).await
}
#[tauri::command]
pub async fn rag_rename(state: State<'_, AppState>, id: String, title: String) -> RagResult<()> {
    let service = service(&state)?;
    blocking(move || service.rename_conversation(&id, &title)).await
}
#[tauri::command]
pub async fn rag_delete(state: State<'_, AppState>, id: String) -> RagResult<()> {
    let service = service(&state)?;
    blocking(move || service.delete_conversation(&id)).await
}
#[tauri::command]
pub async fn rag_conversation(
    state: State<'_, AppState>,
    id: String,
    before_sequence: Option<u64>,
    limit: usize,
) -> RagResult<ConversationPage> {
    service(&state)?
        .conversation(&id, before_sequence, limit)
        .await
}
#[tauri::command]
pub async fn rag_ask(state: State<'_, AppState>, request: RagRequest) -> RagResult<RagAnswer> {
    service(&state)?.ask(request).await
}
#[tauri::command]
pub fn rag_cancel(
    state: State<'_, AppState>,
    conversation_id: String,
    request_id: String,
) -> RagResult<()> {
    service(&state)?.cancel(&conversation_id, &request_id)
}
#[tauri::command]
pub fn rag_job(state: State<'_, AppState>, conversation_id: String) -> RagResult<Option<RagJob>> {
    service(&state)?.job(&conversation_id)
}
#[tauri::command]
pub async fn rag_export(
    state: State<'_, AppState>,
    id: String,
    include_sources: bool,
) -> RagResult<String> {
    service(&state)?.export_markdown(&id, include_sources).await
}
#[tauri::command]
pub async fn rag_sources_hidden(
    state: State<'_, AppState>,
    turn_id: String,
    hidden: bool,
) -> RagResult<()> {
    let service = service(&state)?;
    blocking(move || service.set_sources_hidden(&turn_id, hidden)).await
}
#[tauri::command]
pub async fn generation_credential_status(
    state: State<'_, AppState>,
    slot: ProviderSlot,
    endpoint: String,
) -> RagResult<CredentialStatus> {
    let service = service(&state)?;
    blocking(move || service.credential_status(slot, &endpoint)).await
}
#[tauri::command]
pub async fn generation_set_key(
    state: State<'_, AppState>,
    slot: ProviderSlot,
    endpoint: String,
    key: String,
) -> RagResult<()> {
    let service = service(&state)?;
    let secret = SecretValue::new(key).map_err(|_| RagError::new(RagErrorCode::Input))?;
    service.cancel_all();
    blocking(move || service.set_api_key(slot, &endpoint, &secret)).await
}
#[tauri::command]
pub async fn generation_clear_key(
    state: State<'_, AppState>,
    slot: ProviderSlot,
    endpoint: String,
) -> RagResult<()> {
    let service = service(&state)?;
    service.cancel_all();
    blocking(move || service.clear_api_key(slot, &endpoint)).await
}
