//! Runs the actual production compact catalog module (imported, never copied)
//! so its tests and memory benchmark work on low-memory developer machines.
#![allow(dead_code)]

#[path = "../../../src-tauri/src/filename_compact.rs"]
pub mod filename_compact;
