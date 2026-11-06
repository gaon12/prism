//! Export one saved immutable revision. The live worker, including a codec call
//! that cannot be interrupted internally, retains the shared export admission.
use crate::{
    document_export_file, document_hwpx,
    document_pdf::{self, Code},
    document_pdf_job::Job,
    document_service::{DocumentError, DocumentErrorCode},
    AppState,
};
use serde::Serialize;
use std::{
    fs,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub omitted_images: usize,
    pub literal_html: usize,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
enum CodecCode {
    Hwpx,
    Unsupported,
    TimedOut,
}
#[derive(Serialize)]
pub struct CodecFailure {
    code: CodecCode,
    message: &'static str,
    rag: Option<()>,
}
// Keep the existing document error response for storage/revision/policy errors.
// Codec-specific errors add distinct codes without changing shared document
// service state or reusing a PDF rendering failure for a different format.
#[derive(Serialize)]
#[serde(untagged)]
pub enum Error {
    Document(DocumentError),
    Codec(CodecFailure),
}
type Result<T> = std::result::Result<T, Error>;
impl From<DocumentError> for Error {
    fn from(value: DocumentError) -> Self {
        Self::Document(value)
    }
}
impl From<document_export_file::Error> for Error {
    fn from(value: document_export_file::Error) -> Self {
        Self::Document(DocumentError::new(match value {
            document_export_file::Error::Input => DocumentErrorCode::Input,
            document_export_file::Error::Storage => DocumentErrorCode::Storage,
        }))
    }
}
fn codec(code: CodecCode, message: &'static str) -> Error {
    Error::Codec(CodecFailure {
        code,
        message,
        rag: None,
    })
}
fn operation(code: Code) -> Error {
    if code == Code::TimedOut {
        return codec(CodecCode::TimedOut, "HWPX export exceeded its deadline.");
    }
    DocumentError::new(match code {
        Code::Input => DocumentErrorCode::Input,
        Code::Limit => DocumentErrorCode::Limit,
        Code::Cancelled => DocumentErrorCode::Cancelled,
        Code::Busy => DocumentErrorCode::Busy,
        Code::Storage => DocumentErrorCode::Storage,
        Code::Render | Code::TimedOut => return codec(CodecCode::Hwpx, "HWPX export failed."),
    })
    .into()
}
fn encode(code: document_hwpx::Code) -> Error {
    match code {
        document_hwpx::Code::Input => operation(Code::Input),
        document_hwpx::Code::Limit => operation(Code::Limit),
        document_hwpx::Code::Cancelled => operation(Code::Cancelled),
        document_hwpx::Code::TimedOut => operation(Code::TimedOut),
        document_hwpx::Code::Unsupported => codec(
            CodecCode::Unsupported,
            "This document contains formatting that the HWPX exporter cannot preserve.",
        ),
        document_hwpx::Code::Encode => codec(
            CodecCode::Hwpx,
            "The selected revision could not be encoded as HWPX.",
        ),
    }
}

#[tauri::command]
#[allow(clippy::too_many_arguments)] // Explicit immutable revision/options IPC.
pub async fn doc_export_hwpx(
    state: tauri::State<'_, AppState>,
    window: tauri::WebviewWindow,
    id: String,
    revision: String,
    include_sources: bool,
    include_type_markers: bool,
    request: String,
    path: String,
) -> Result<Summary> {
    if window.label() != "main" {
        return Err(operation(Code::Input));
    }
    let path = PathBuf::from(path);
    document_export_file::validate(&path, "hwpx")?;
    let service = state.documents.clone()?;
    let language = state
        .settings
        .lock()
        .map_err(|_| operation(Code::Storage))?
        .language
        .clone();
    let job = Job::start(&request, window.label()).map_err(operation)?;
    let worker = Arc::clone(&job);
    let mut task = tauri::async_runtime::spawn_blocking(move || -> Result<Summary> {
        worker.control.check().map_err(operation)?;
        let page = service.get(&id, Some(&revision))?;
        let markdown =
            service.export_markdown(&id, Some(&revision), include_sources, include_type_markers)?;
        let control = document_hwpx::Control {
            cancel: worker.control.cancel.as_ref(),
            deadline: worker.control.deadline,
        };
        let export =
            document_hwpx::render(&page.revision.plan.title, &markdown, &language, &control)
                .map_err(|code| {
                    worker
                        .cancellation()
                        .map_or_else(|| encode(code), operation)
                })?;
        document_export_file::save(
            &path,
            "hwpx",
            &export.bytes,
            32 << 20,
            || {
                worker.control.check().map_err(operation)?;
                service
                    .ensure_external(&page.revision.provenance)
                    .map_err(Error::from)
            },
            |stage, destination| {
                let result = worker.commit(|| {
                    service
                        .ensure_external(&page.revision.provenance)
                        .map_err(|_| Code::Input)?;
                    fs::rename(stage, destination).map_err(|_| Code::Storage)
                });
                if let Err(code) = result {
                    service.ensure_external(&page.revision.provenance)?;
                    return Err(operation(code));
                }
                Ok(())
            },
        )?;
        Ok(Summary {
            omitted_images: export.omitted_images,
            literal_html: export.literal_html,
        })
    });
    loop {
        tokio::select! {
            result = &mut task => return result.map_err(|_| encode(document_hwpx::Code::Encode))?,
            _ = tokio::time::sleep(Duration::from_millis(25)) => {
                if let Some(reason) = job.cancellation() {
                    return Err(operation(reason));
                }
                if Instant::now() >= job.control.deadline && job.stop(document_pdf::Code::TimedOut) {
                    return Err(operation(Code::TimedOut));
                }
            }
        }
    }
}

#[tauri::command]
pub fn doc_cancel_hwpx(window: tauri::WebviewWindow, request: String) -> bool {
    window.label() == "main" && Job::cancel(&request, window.label())
}
