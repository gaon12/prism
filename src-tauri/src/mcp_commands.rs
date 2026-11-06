//! Settings-screen commands for external sources through MCP (requirement Q45).
//!
//! `mcp_probe` starts one *saved* server (looked up by id in the saved settings, never a
//! program named by the caller), performs the handshake, lists its tools and stops it again.
//! Only the main window may call it, one probe at a time, with a 15 s limit. Nothing about
//! the server's output is logged.
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use tauri::State;

use crate::{
    mcp_client::{Client, McpError, Tool},
    AppState,
};

static BUSY: AtomicBool = AtomicBool::new(false);

/// Clears the busy flag when the probe ends, even after a panic.
struct Admission;
impl Drop for Admission {
    fn drop(&mut self) {
        BUSY.store(false, Ordering::Release);
    }
}

/// Short codes for the settings screen.
fn code(error: McpError) -> String {
    match error {
        McpError::Start => "start",
        McpError::TimedOut => "timedOut",
        McpError::Cancelled => "cancelled",
        McpError::Closed => "closed",
        McpError::Protocol => "protocol",
        McpError::Server(_) => "server",
        McpError::NotAllowed => "notAllowed",
        McpError::Schema(_) => "schema",
    }
    .to_string()
}

#[tauri::command]
pub async fn mcp_probe(
    window: tauri::Window,
    state: State<'_, AppState>,
    server_id: String,
) -> Result<Vec<Tool>, String> {
    if window.label() != "main" {
        return Err("input".into());
    }
    let config = {
        let settings = state.settings.lock().map_err(|_| "input".to_string())?;
        settings
            .mcp
            .servers
            .iter()
            .find(|server| server.id == server_id)
            .cloned()
            .ok_or_else(|| "notFound".to_string())?
    };
    BUSY.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .map_err(|_| "busy".to_string())?;
    let admission = Admission;
    tauri::async_runtime::spawn_blocking(move || {
        let _admission = admission;
        let cancel = Arc::new(AtomicBool::new(false));
        let mut client = Client::connect(&config, Duration::from_secs(15), cancel).map_err(code)?;
        client.list_tools().map_err(code)
        // Dropping the client here stops the server and its child processes.
    })
    .await
    .map_err(|_| "closed".to_string())?
}
