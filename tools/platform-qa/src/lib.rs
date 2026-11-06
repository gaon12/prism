//! Compile the exact production platform modules without Tauri, WebView or Lance.
#[path = "../../../src-tauri/src/collection_epub.rs"]
pub mod collection_epub;
#[path = "../../../src-tauri/src/collection_import.rs"]
pub mod collection_import;
#[path = "../../../src-tauri/src/collection_render.rs"]
pub mod collection_render;
#[path = "../../../src-tauri/src/collection_service.rs"]
pub mod collection_service;
#[path = "../../../src-tauri/src/collection_store.rs"]
pub mod collection_store;
#[path = "../../../src-tauri/src/document_export_file.rs"]
pub mod document_export_file;
#[path = "../../../src-tauri/src/document_pdf.rs"]
pub mod document_pdf;
#[path = "../../../src-tauri/src/document_pdf_file.rs"]
pub mod document_pdf_file;
#[path = "../../../src-tauri/src/document_pdf_job.rs"]
pub mod document_pdf_job;
#[path = "../../../src-tauri/src/ocr.rs"]
pub mod ocr;
#[path = "../../../src-tauri/src/ocr_assets.rs"]
pub mod ocr_assets;
#[path = "../../../src-tauri/src/ocr_package.rs"]
pub mod ocr_package;
#[path = "../../../src-tauri/src/ocr_runtime.rs"]
pub mod ocr_runtime;
#[path = "../../../src-tauri/src/office_unlock.rs"]
pub mod office_unlock;
#[cfg(target_os = "linux")]
#[path = "../../../src-tauri/src/pdf_renderer.rs"]
pub mod pdf_renderer;
// Full-app callers are absent here. Keep the existing source modules' style
// lint baseline local to this harness; the application still reports its lints.
#[allow(dead_code, clippy::cloned_ref_to_slice_refs)]
#[path = "../../../src-tauri/src/content_store.rs"]
pub mod content_store;
#[allow(
    clippy::inconsistent_digit_grouping,
    clippy::manual_is_multiple_of,
    clippy::chunks_exact_to_as_chunks,
    clippy::needless_range_loop,
    clippy::too_many_arguments
)]
#[path = "../../../src-tauri/src/document_extract.rs"]
pub mod document_extract;
#[path = "../../../src-tauri/src/embedding_assets.rs"]
pub mod embedding_assets;
#[path = "../../../src-tauri/src/embedding_package.rs"]
pub mod embedding_package;
#[path = "../../../src-tauri/src/embedding_runtime.rs"]
pub mod embedding_runtime;
// This helper's production callers live in the full app; this crate runs its tests.
#[allow(dead_code)]
#[path = "../../../src-tauri/src/path_identity.rs"]
pub mod path_identity;
#[path = "../../../src-tauri/src/power.rs"]
pub mod power;
#[path = "../../../src-tauri/src/process_job.rs"]
pub mod process_job;
#[path = "../../../src-tauri/src/runtime_process.rs"]
pub mod runtime_process;
#[path = "../../../src-tauri/src/runtime_supervisor.rs"]
pub mod runtime_supervisor;
#[path = "../../../src-tauri/src/secret_store.rs"]
pub mod secret_store;
#[allow(dead_code)]
#[path = "../../../src-tauri/src/unlock_requests.rs"]
pub mod unlock_requests;
