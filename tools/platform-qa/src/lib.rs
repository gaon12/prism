//! Compile the exact production platform modules without Tauri, WebView or Lance.
#[path = "../../../src-tauri/src/embedding_assets.rs"]
pub mod embedding_assets;
#[path = "../../../src-tauri/src/embedding_package.rs"]
pub mod embedding_package;
#[path = "../../../src-tauri/src/embedding_runtime.rs"]
pub mod embedding_runtime;
#[path = "../../../src-tauri/src/process_job.rs"]
pub mod process_job;
#[path = "../../../src-tauri/src/secret_store.rs"]
pub mod secret_store;
