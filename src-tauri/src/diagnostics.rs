//! A user-requested diagnostic snapshot, built from an explicit allowlist.
//! Never serialize application state or error objects directly: they can contain
//! document text, paths, endpoints, credential identifiers or remote responses.
use crate::{app_paths::InstallKind, AppState};
use serde::Serialize;
use tauri::Manager;
use tauri_plugin_opener::OpenerExt;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticSnapshot {
    schema_version: u32,
    generated_at: String,
    app: AppSummary,
    settings: Option<SettingsSummary>,
    health: HealthSummary,
    filename_index: Option<FilenameSummary>,
    content_index: Option<ContentSummary>,
    embedding: Option<EmbeddingSummary>,
    clipboard: Option<ClipboardSummary>,
    updates: Option<UpdateSummary>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AppSummary {
    version: &'static str,
    os: &'static str,
    architecture: &'static str,
    development: bool,
    distribution: InstallKind,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsSummary {
    filename_scope_count: usize,
    content_scope_count: usize,
    external_scope_count: usize,
    generation_mode: crate::rag_service::GenerationMode,
    fallback_enabled: bool,
}
impl From<&crate::settings::ClientSettings> for SettingsSummary {
    fn from(settings: &crate::settings::ClientSettings) -> Self {
        Self {
            filename_scope_count: settings
                .scopes
                .iter()
                .filter(|s| s.filename_enabled)
                .count(),
            content_scope_count: settings.scopes.iter().filter(|s| s.content_enabled).count(),
            external_scope_count: settings
                .scopes
                .iter()
                .filter(|s| s.external_allowed)
                .count(),
            generation_mode: settings.generation.mode,
            fallback_enabled: settings.generation.fallback,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HealthSummary {
    // None means the lock could not be read, not that the component is healthy.
    settings_error: Option<bool>,
    shortcut_error: Option<bool>,
    startup_error: Option<bool>,
    dictionary_initialized: bool,
    embedding_initialized: bool,
    content_initialized: bool,
    chat_initialized: bool,
    documents_initialized: bool,
    updates_initialized: bool,
}

fn error_flag(error: &std::sync::Mutex<Option<String>>) -> Option<bool> {
    error.try_lock().ok().map(|value| value.is_some())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FilenameSummary {
    scanning: bool,
    count: usize,
    skipped: usize,
    warning_count: usize,
    watching: bool,
    last_service_snapshot: Option<crate::index::ServiceSnapshotTiming>,
    service_snapshots: u32,
    overflow_reconciles: u32,
}
impl From<crate::index::IndexStatus> for FilenameSummary {
    fn from(status: crate::index::IndexStatus) -> Self {
        Self {
            scanning: status.scanning,
            count: status.count,
            skipped: status.skipped,
            warning_count: status.warnings.len(),
            watching: status.watching,
            last_service_snapshot: status.last_service_snapshot,
            service_snapshots: status.service_snapshots,
            overflow_reconciles: status.overflow_reconciles,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ContentSummary {
    phase: &'static str,
    discovered: usize,
    indexed: usize,
    skipped: usize,
    embedded: usize,
    issue_count: usize,
    embedding_error: bool,
}
impl From<crate::content_service::ContentStatus> for ContentSummary {
    fn from(status: crate::content_service::ContentStatus) -> Self {
        Self {
            // Phase is currently a String in the service. Do not copy arbitrary text.
            phase: match status.phase.as_str() {
                "ready" => "ready",
                "scanning" => "scanning",
                "indexing" => "indexing",
                "embedding" => "embedding",
                "paused" => "paused",
                "error" => "error",
                "stopped" => "stopped",
                "cancelled" => "cancelled",
                _ => "unknown",
            },
            discovered: status.discovered,
            indexed: status.indexed,
            skipped: status.skipped,
            embedded: status.embedded,
            issue_count: status.issues.len(),
            embedding_error: status.embedding_error.is_some(),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EmbeddingSummary {
    phase: crate::embedding_runtime::EmbeddingPhase,
    model_ready: bool,
    runtime_ready: bool,
    dimensions: usize,
    error: bool,
}
impl From<crate::embedding_runtime::EmbeddingStatus> for EmbeddingSummary {
    fn from(status: crate::embedding_runtime::EmbeddingStatus) -> Self {
        Self {
            phase: status.phase,
            model_ready: status.model_ready,
            runtime_ready: status.runtime_ready,
            dimensions: status.dimensions,
            error: status.last_error.is_some(),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ClipboardSummary {
    enabled: bool,
    paused: bool,
    count: usize,
    bytes: u64,
    error: bool,
}
impl From<crate::clipboard_history::ClipStatus> for ClipboardSummary {
    fn from(status: crate::clipboard_history::ClipStatus) -> Self {
        Self {
            enabled: status.enabled,
            paused: status.paused,
            count: status.count,
            bytes: status.bytes,
            error: status.last_error.is_some(),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateSummary {
    auto_check: bool,
    checking: bool,
    installing: bool,
    can_install: bool,
    check_error: bool,
    install_error: bool,
    state_error: bool,
}
impl From<crate::update_runtime::UpdateView> for UpdateSummary {
    fn from(view: crate::update_runtime::UpdateView) -> Self {
        Self {
            auto_check: view.status.auto_check,
            checking: view.status.checking,
            installing: view.status.installing,
            can_install: view.status.can_install,
            check_error: view.status.check_error.is_some(),
            install_error: view.status.install_error.is_some(),
            state_error: view.state_error,
        }
    }
}

fn snapshot(state: &AppState) -> DiagnosticSnapshot {
    DiagnosticSnapshot {
        schema_version: 1,
        generated_at: chrono::Utc::now().to_rfc3339(),
        app: AppSummary {
            version: env!("CARGO_PKG_VERSION"),
            os: std::env::consts::OS,
            architecture: std::env::consts::ARCH,
            development: cfg!(debug_assertions),
            distribution: state.location.kind,
        },
        settings: state
            .settings
            .try_lock()
            .ok()
            .map(|s| SettingsSummary::from(&*s)),
        health: HealthSummary {
            settings_error: error_flag(&state.config_error),
            shortcut_error: error_flag(&state.shortcut_error),
            startup_error: error_flag(&state.startup_error),
            dictionary_initialized: state.dictionary.is_ok(),
            embedding_initialized: state.embedding.is_ok(),
            content_initialized: state.content.is_ok(),
            chat_initialized: state.rag.is_ok(),
            documents_initialized: state.documents.is_ok(),
            updates_initialized: state.updates.is_ok(),
        },
        filename_index: state.index.status().ok().map(Into::into),
        content_index: state
            .content
            .as_ref()
            .ok()
            .and_then(|s| s.status().ok())
            .map(Into::into),
        embedding: state.embedding.as_ref().ok().map(|s| s.status().into()),
        clipboard: state.clipboard.status().ok().map(Into::into),
        updates: state.updates.as_ref().ok().map(|s| s.status().into()),
    }
}

#[tauri::command]
pub async fn diagnostics_snapshot(app: tauri::AppHandle) -> Result<DiagnosticSnapshot, String> {
    // Status getters can acquire service locks. Keep them off the UI thread.
    // They do not start a model, run a search, read documents or make network calls.
    tauri::async_runtime::spawn_blocking(move || snapshot(&app.state::<AppState>()))
        .await
        .map_err(|_| "Diagnostic information is unavailable. Try again.".into())
}

#[tauri::command]
pub fn diagnostics_open_issue(app: tauri::AppHandle) -> Result<(), String> {
    // Fixed URL, no query string: no report data is placed in browser history or
    // sent to GitHub by this command. The user pastes and submits it themselves.
    app.opener()
        .open_url("https://github.com/gaon12/prism/issues/new", None::<&str>)
        .map_err(|_| "Could not open the issue page.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    const PRIVATE: &str = "PRIVATE:/Users/sensitive/document API-KEY request body";

    #[test]
    fn diagnostic_projections_drop_paths_messages_and_remote_metadata() {
        let filename = FilenameSummary::from(crate::index::IndexStatus {
            warnings: vec![PRIVATE.into()],
            count: 12,
            last_service_snapshot: Some(crate::index::ServiceSnapshotTiming {
                received_records: 12,
                first_batch_ms: Some(30),
                validated_fetch_ms: 50,
                receive_callback_ms: 5,
                finish_publish_ms: 10,
                total_ms: 60,
                initial_catalog: true,
            }),
            service_snapshots: 2,
            overflow_reconciles: 1,
            ..Default::default()
        });
        let content = ContentSummary::from(crate::content_service::ContentStatus {
            phase: PRIVATE.into(),
            current_path: Some(PRIVATE.into()),
            embedding_error: Some(PRIVATE.into()),
            issues: vec![crate::content_service::ContentIssue {
                path: PRIVATE.into(),
                code: PRIVATE.into(),
                message: PRIVATE.into(),
            }],
            ..Default::default()
        });
        let clipboard = ClipboardSummary::from(crate::clipboard_history::ClipStatus {
            last_error: Some(PRIVATE.into()),
            count: 3,
            ..Default::default()
        });
        let embedding = EmbeddingSummary::from(crate::embedding_runtime::EmbeddingStatus {
            phase: crate::embedding_runtime::EmbeddingPhase::Error,
            model_id: PRIVATE.into(),
            model_revision: PRIVATE.into(),
            runtime_version: PRIVATE.into(),
            runtime_target: Some(PRIVATE.into()),
            runtime_filename: Some(PRIVATE.into()),
            runtime_bytes: Some(1),
            quantization: Some(PRIVATE.into()),
            model_ready: true,
            runtime_ready: true,
            downloaded_bytes: 0,
            total_bytes: 0,
            pid: Some(123),
            dimensions: 768,
            last_error: Some(PRIVATE.into()),
        });
        let updates = UpdateSummary::from(crate::update_runtime::UpdateView {
            current_version: PRIVATE,
            download_page: PRIVATE,
            install_disabled_reason: Some(PRIVATE),
            state_error: true,
            status: crate::update_service::UpdateStatus {
                available: Some(crate::update_service::AvailableUpdate {
                    version: PRIVATE.into(),
                    notes: PRIVATE.into(),
                }),
                dismissed_version: Some(PRIVATE.into()),
                check_error: Some(PRIVATE.into()),
                install_error: Some(PRIVATE.into()),
                ..Default::default()
            },
        });
        let data = serde_json::json!({"filename": filename, "content": content,
            "clipboard": clipboard, "embedding": embedding, "updates": updates});
        assert!(!data.to_string().contains("PRIVATE"));
        assert_eq!(data["content"]["phase"], "unknown");
        assert_eq!(data["content"]["issueCount"], 1);
        assert_eq!(data["filename"]["warningCount"], 1);
        assert_eq!(data["filename"]["lastServiceSnapshot"]["totalMs"], 60);
        assert_eq!(data["filename"]["serviceSnapshots"], 2);
        assert_eq!(data["filename"]["overflowReconciles"], 1);
        assert_eq!(data["embedding"]["error"], true);
        assert_eq!(data["updates"]["installError"], true);
    }

    #[test]
    fn settings_projection_only_counts_scopes_and_copies_routing_enum() {
        let mut settings = crate::settings::ClientSettings {
            shortcut: PRIVATE.into(),
            language: PRIVATE.into(),
            generation: serde_json::from_value(serde_json::json!({
                "mode": "auto", "fallback": true,
                "local": { "endpoint": PRIVATE, "model": PRIVATE, "maxTokens": 32,
                    "tokenParameter": "maxTokens", "temperature": null }
            }))
            .unwrap(),
            ..Default::default()
        };
        settings.scopes.push(crate::settings::SearchScope {
            id: PRIVATE.into(),
            name: PRIVATE.into(),
            path: PRIVATE.into(),
            filename_enabled: true,
            content_enabled: true,
            external_allowed: false,
        });
        let data = serde_json::to_value(SettingsSummary::from(&settings)).unwrap();
        assert!(!data.to_string().contains("PRIVATE"));
        assert_eq!(data["filenameScopeCount"], 1);
        assert_eq!(data["contentScopeCount"], 1);
        assert_eq!(data["externalScopeCount"], 0);
        assert_eq!(data["generationMode"], "auto");
        assert_eq!(data["fallbackEnabled"], true);
    }

    #[test]
    fn unreadable_error_state_is_unknown_instead_of_healthy() {
        let state = std::sync::Mutex::new(Some(PRIVATE.to_owned()));
        assert_eq!(error_flag(&state), Some(true));
        let guard = state.lock().unwrap();
        assert_eq!(error_flag(&state), None);
        drop(guard);
        *state.lock().unwrap() = None;
        assert_eq!(error_flag(&state), Some(false));
    }
}
