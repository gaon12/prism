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

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PdfExportSummary {
    pub pages: usize,
    pub omitted_images: usize,
}
fn pdf_error(code: crate::document_pdf::Code) -> DocumentError {
    use crate::document_pdf::Code;
    DocumentError::new(match code {
        Code::Busy => DocumentErrorCode::Busy,
        Code::Input => DocumentErrorCode::Input,
        Code::Limit => DocumentErrorCode::Limit,
        Code::Cancelled => DocumentErrorCode::Cancelled,
        Code::TimedOut => DocumentErrorCode::TimedOut,
        Code::Storage => DocumentErrorCode::Storage,
        Code::Render => DocumentErrorCode::Pdf,
    })
}
#[tauri::command]
// Keep the explicit revision/options/request fields in the native IPC contract.
#[allow(clippy::too_many_arguments)]
pub async fn doc_export_pdf(
    state: State<'_, AppState>,
    window: tauri::WebviewWindow,
    id: String,
    revision: String,
    include_sources: bool,
    include_type_markers: bool,
    request: String,
    path: String,
) -> DocumentResult<PdfExportSummary> {
    use crate::{document_pdf, document_pdf_file, document_pdf_job::Job};
    use std::{
        path::PathBuf,
        sync::Arc,
        time::{Duration, Instant},
    };
    if window.label() != "main" {
        return Err(DocumentError::new(DocumentErrorCode::Input));
    }
    let path = PathBuf::from(path);
    document_pdf_file::validate(&path).map_err(pdf_error)?;
    let service = service(&state)?;
    let language = state.settings.lock().map_err(storage)?.language.clone();
    let job = Job::start(&request, window.label()).map_err(pdf_error)?;
    let worker = Arc::clone(&job);
    let mut task = tauri::async_runtime::spawn_blocking(move || {
        // The backend supplies the saved immutable version, never client layout.
        let page = service.get(&id, Some(&revision))?;
        let markdown =
            service.export_markdown(&id, Some(&revision), include_sources, include_type_markers)?;
        let pdf = document_pdf::render(
            &page.revision.plan.title,
            &markdown,
            &language,
            &worker.control,
        )
        .map_err(|code| pdf_error(worker.cancellation().unwrap_or(code)))?;
        document_pdf_file::save(&path, &pdf, &worker).map_err(pdf_error)?;
        Ok(PdfExportSummary {
            pages: pdf.pages,
            omitted_images: pdf.omitted_images,
        })
    });
    loop {
        tokio::select! {
            result=&mut task=>return result.map_err(|_|DocumentError::new(DocumentErrorCode::Pdf))?,
            _=tokio::time::sleep(Duration::from_millis(25))=>{
                if let Some(reason)=job.cancellation(){return Err(pdf_error(reason));}
                if Instant::now()>=job.control.deadline && job.stop(document_pdf::Code::TimedOut) {
                    return Err(pdf_error(document_pdf::Code::TimedOut));
                }
                // Cancellation is too late once the atomic replacement starts.
            }
        }
    }
}
#[tauri::command]
pub fn doc_cancel_pdf(window: tauri::WebviewWindow, request: String) -> bool {
    if window.label() != "main" {
        return false;
    }
    crate::document_pdf_job::Job::cancel(&request, window.label())
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
