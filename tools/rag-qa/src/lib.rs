//! Run the actual RAG/document policy and persistence modules without linking
//! the GUI, index engines, vector runtime or document export renderers.
//! Engine-to-snapshot adapters stay in the full app and need its integration
//! tests. These modules are imported directly, never copied or substituted.
// Full-app entry points intentionally have no callers in this test-only crate.
#![allow(dead_code)]

#[path = "../../../src-tauri/src/chat_selection.rs"]
mod chat_selection;
#[path = "../../../src-tauri/src/document_schema.rs"]
mod document_schema;
#[path = "../../../src-tauri/src/document_service.rs"]
mod document_service;
#[path = "../../../src-tauri/src/document_store.rs"]
mod document_store;
#[path = "../../../src-tauri/src/generation_runtime.rs"]
mod generation_runtime;
#[path = "../../../src-tauri/src/mcp_client.rs"]
mod mcp_client;
#[cfg(windows)]
#[path = "../../../src-tauri/src/process_job.rs"]
mod process_job;
#[path = "../../../src-tauri/src/rag_external.rs"]
mod rag_external;
#[path = "../../../src-tauri/src/rag_service.rs"]
mod rag_service;
#[path = "../../../src-tauri/src/rag_store.rs"]
mod rag_store;
#[path = "../../../src-tauri/src/secret_store.rs"]
mod secret_store;
#[path = "../../../src-tauri/src/selection_set.rs"]
mod selection_set;
