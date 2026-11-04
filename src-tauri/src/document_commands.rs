use crate::{document_service::*, rag_service::RagCitation, AppState};
use tauri::State;
fn service(state: &AppState) -> DocumentResult<DocumentService> {
    state.documents.clone()
}
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> DocumentResult<T> + Send + 'static,
) -> DocumentResult<T> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|_| DocumentError::new(DocumentErrorCode::Storage))?
}
#[tauri::command]
pub async fn doc_create(
    state: State<'_, AppState>,
    input: CreateDocument,
) -> DocumentResult<DocumentPage> {
    let service = service(&state)?;
    blocking(move || service.create(input)).await
}
#[tauri::command]
pub async fn doc_list(
    state: State<'_, AppState>,
    query: String,
    limit: usize,
) -> DocumentResult<Vec<DocumentSummary>> {
    let service = service(&state)?;
    blocking(move || service.list(&query, limit)).await
}
#[tauri::command]
pub async fn doc_get(
    state: State<'_, AppState>,
    id: String,
    revision_id: Option<String>,
) -> DocumentResult<DocumentPage> {
    let service = service(&state)?;
    blocking(move || service.get(&id, revision_id.as_deref())).await
}
#[tauri::command]
pub async fn doc_current_citations(
    state: State<'_, AppState>,
    id: String,
    revision_id: Option<String>,
) -> DocumentResult<Vec<RagCitation>> {
    service(&state)?
        .current_citations(&id, revision_id.as_deref())
        .await
}
#[tauri::command]
pub async fn doc_revisions(
    state: State<'_, AppState>,
    id: String,
    before: Option<u64>,
    limit: usize,
) -> DocumentResult<Vec<DocumentRevisionSummary>> {
    let service = service(&state)?;
    blocking(move || service.revisions(&id, before, limit)).await
}
#[tauri::command]
pub async fn doc_rename(
    state: State<'_, AppState>,
    id: String,
    value: String,
) -> DocumentResult<()> {
    let service = service(&state)?;
    blocking(move || service.rename(&id, &value)).await
}
#[tauri::command]
pub async fn doc_delete(state: State<'_, AppState>, id: String) -> DocumentResult<()> {
    let service = service(&state)?;
    blocking(move || service.delete(&id)).await
}
#[tauri::command]
pub async fn doc_save_outline(
    state: State<'_, AppState>,
    id: String,
    expected: String,
    plan: DocumentPlan,
    reviewed: bool,
) -> DocumentResult<DocumentRevision> {
    let service = service(&state)?;
    blocking(move || service.save_outline(&id, &expected, plan, reviewed)).await
}
#[tauri::command]
pub async fn doc_save_manual(
    state: State<'_, AppState>,
    id: String,
    expected: String,
    blocks: Vec<DocumentBlock>,
    plan: Option<DocumentPlan>,
) -> DocumentResult<DocumentRevision> {
    let service = service(&state)?;
    blocking(move || service.save_manual(&id, &expected, blocks, plan)).await
}
#[tauri::command]
pub async fn doc_restore(
    state: State<'_, AppState>,
    id: String,
    expected: String,
    target: String,
) -> DocumentResult<DocumentRevision> {
    let service = service(&state)?;
    blocking(move || service.restore(&id, &expected, &target)).await
}
#[tauri::command]
pub async fn doc_export(
    state: State<'_, AppState>,
    id: String,
    revision: Option<String>,
    include_sources: bool,
    include_type_markers: bool,
) -> DocumentResult<String> {
    let service = service(&state)?;
    blocking(move || {
        service.export_markdown(
            &id,
            revision.as_deref(),
            include_sources,
            include_type_markers,
        )
    })
    .await
}
#[tauri::command]
pub async fn doc_generate_outline(
    state: State<'_, AppState>,
    request: DocumentGenerateRequest,
) -> DocumentResult<DocumentRevision> {
    service(&state)?.generate_outline(request).await
}
#[tauri::command]
pub async fn doc_generate_body(
    state: State<'_, AppState>,
    request: DocumentGenerateRequest,
) -> DocumentResult<DocumentRevision> {
    service(&state)?.generate_body(request).await
}
#[tauri::command]
pub async fn doc_rewrite(
    state: State<'_, AppState>,
    request: DocumentRewriteRequest,
) -> DocumentResult<DocumentRevision> {
    service(&state)?.rewrite(request).await
}
#[tauri::command]
pub fn doc_cancel(state: State<'_, AppState>, id: String, request: String) -> DocumentResult<()> {
    service(&state)?.cancel(&id, &request)
}
#[tauri::command]
pub fn doc_job(state: State<'_, AppState>, id: String) -> DocumentResult<Option<DocumentJob>> {
    service(&state)?.job(&id)
}
