//! Compile the exact production platform modules without Tauri, WebView or Lance.
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
#[path = "../../../src-tauri/src/process_job.rs"]
pub mod process_job;
#[path = "../../../src-tauri/src/runtime_process.rs"]
pub mod runtime_process;
#[path = "../../../src-tauri/src/runtime_supervisor.rs"]
pub mod runtime_supervisor;
#[path = "../../../src-tauri/src/secret_store.rs"]
pub mod secret_store;
