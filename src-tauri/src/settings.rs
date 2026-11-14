use serde::{Deserialize, Serialize};
use std::{collections::HashSet, fs, path::Path};
use tauri_plugin_global_shortcut::Shortcut;

#[path = "file_shortcut.rs"]
mod file_shortcut;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchScope {
    pub id: String,
    pub name: String,
    pub path: String,
    pub filename_enabled: bool,
    pub content_enabled: bool,
    #[serde(default)]
    pub external_allowed: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ClientSettings {
    pub schema_version: u32,
    pub revision: u64,
    #[serde(default = "legacy_onboarding_version")]
    pub onboarding_version: u32,
    pub theme: String,
    pub language: String,
    pub font_size: u8,
    pub shortcut: String,
    pub file_open_shortcut: String,
    pub remember_search: bool,
    pub keyboard_correction: bool,
    pub hybrid_search: bool,
    pub automatic_semantic: bool,
    pub close_to_tray: bool,
    pub start_on_login: bool,
    pub scopes: Vec<SearchScope>,
    pub file_policy: crate::file_policy::Preferences,
    pub calculator: CalculatorSettings,
    pub clipboard: crate::clipboard_history::ClipboardSettings,
    pub generation: crate::rag_service::GenerationPreferences,
    pub updates: UpdatePreferences,
    pub ocr: crate::ocr::Preferences,
    pub power: PowerPreferences,
    pub documents: DocumentPreferences,
    pub mcp: McpPreferences,
}

/// External sources through MCP servers (requirement Q45). Off by default.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct McpPreferences {
    /// Master switch. Even when on, a question is only sent to a server under the same
    /// external-sending permissions as RAG and after the user opts in for that question.
    pub enabled: bool,
    pub servers: Vec<crate::mcp_client::ServerConfig>,
}

impl McpPreferences {
    /// Drops half-filled rows from the settings screen (no program yet) and names unnamed
    /// servers after their program, so saving never fails because of an empty new row.
    pub fn tidy(&mut self) {
        self.servers
            .retain(|server| !server.command.trim().is_empty());
        for server in &mut self.servers {
            server.args.retain(|arg| !arg.is_empty());
            if server.name.trim().is_empty() {
                server.name = std::path::Path::new(server.command.trim())
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().chars().take(80).collect())
                    .unwrap_or_else(|| "MCP".into());
            }
        }
    }

    /// Rejects anything a hand-edited settings file could use to start unexpected programs
    /// or to overload the settings screen.
    pub fn validate(&self) -> Result<(), String> {
        let fail = || Err("Invalid external source (MCP) settings.".to_string());
        if self.servers.len() > crate::mcp_client::MAX_SERVERS {
            return fail();
        }
        let mut ids = HashSet::new();
        for server in &self.servers {
            let id_ok = !server.id.is_empty()
                && server.id.len() <= 64
                && server
                    .id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
            let text_ok = |text: &str, max: usize| text.len() <= max && !text.contains('\0');
            let mapping_ok = server.search.as_ref().is_none_or(|mapping| {
                !mapping.tool.is_empty()
                    && !mapping.argument.is_empty()
                    && text_ok(&mapping.tool, 200)
                    && text_ok(&mapping.argument, 200)
            });
            if !id_ok
                || !ids.insert(server.id.as_str())
                || server.name.trim().is_empty()
                || !text_ok(&server.name, 80)
                || server.command.trim().is_empty()
                || !text_ok(&server.command, 1024)
                || server.args.len() > 32
                || !server.args.iter().all(|arg| text_ok(arg, 4096))
                || server.allowed_tools.len() > 256
                || !server.allowed_tools.iter().all(|tool| text_ok(tool, 200))
                || !mapping_ok
            {
                return fail();
            }
        }
        Ok(())
    }
}

/// What the Documents tab opens with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DocumentMode {
    /// Write a new document (the default).
    #[default]
    New,
    /// Fill an existing DOCX/HWPX form.
    Fill,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DocumentPreferences {
    pub default_mode: DocumentMode,
}

/// Battery behaviour. Off by default: content indexing runs on battery unless the user asks.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PowerPreferences {
    /// Wait for AC power before starting new content indexing (text extraction, OCR,
    /// embeddings). Filename indexing and searching are not affected.
    pub pause_on_battery: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UpdatePreferences {
    pub auto_check: bool,
}
impl Default for UpdatePreferences {
    fn default() -> Self {
        Self { auto_check: true }
    }
}

// Existing profiles predate the tour; only genuinely new settings start at zero.
fn legacy_onboarding_version() -> u32 {
    1
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct CalculatorSettings {
    pub precision: u8,
    pub angle: String,
}
impl Default for CalculatorSettings {
    fn default() -> Self {
        Self {
            precision: 12,
            angle: "rad".into(),
        }
    }
}

impl Default for ClientSettings {
    fn default() -> Self {
        Self {
            schema_version: 1,
            revision: 0,
            onboarding_version: 0,
            theme: "system".into(),
            language: "ko".into(),
            font_size: 14,
            shortcut: "Alt+Space".into(),
            file_open_shortcut: "Enter".into(),
            remember_search: true,
            keyboard_correction: true,
            hybrid_search: true,
            automatic_semantic: true,
            close_to_tray: true,
            start_on_login: true,
            scopes: vec![],
            file_policy: crate::file_policy::Preferences::default(),
            calculator: CalculatorSettings::default(),
            clipboard: crate::clipboard_history::ClipboardSettings::default(),
            generation: crate::rag_service::GenerationPreferences::default(),
            updates: UpdatePreferences::default(),
            ocr: crate::ocr::Preferences::default(),
            power: PowerPreferences::default(),
            documents: DocumentPreferences::default(),
            mcp: McpPreferences::default(),
        }
    }
}

impl ClientSettings {
    /// Persist a stable long spelling on explicit save. A folder selected via an
    /// 8.3 alias can then go offline without losing its cached root identity.
    /// Linked or unavailable inputs remain unchanged and are not made readable.
    pub(crate) fn normalize_scope_paths(&mut self) {
        for scope in &mut self.scopes {
            if let Ok(path) = crate::path_identity::canonical(Path::new(&scope.path)) {
                if path.is_dir() {
                    if let Some(path) = crate::path_identity::normal(&path).to_str() {
                        scope.path = path.to_owned();
                    }
                }
            }
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        self.ocr.validate().map_err(|value| value.message)?;
        self.clipboard.validate()?;
        self.mcp.validate()?;
        self.generation.validate().map_err(|error| error.message)?;
        if !(6..=15).contains(&self.calculator.precision)
            || !["rad", "deg"].contains(&self.calculator.angle.as_str())
        {
            return Err("Invalid calculator settings.".into());
        }
        if self.schema_version != 1 {
            return Err(
                "Unsupported settings version. Your existing file has not been changed.".into(),
            );
        }
        if !["system", "light", "dark"].contains(&self.theme.as_str()) {
            return Err("Invalid theme.".into());
        }
        if !["ko", "en", "zh-CN", "zh-TW", "ja"].contains(&self.language.as_str()) {
            return Err("Invalid language.".into());
        }
        if !(12..=18).contains(&self.font_size) {
            return Err("Text size must be between 12 and 18.".into());
        }
        if !self.shortcut.contains('+') {
            return Err("The global shortcut must include a modifier key.".into());
        }
        let global_shortcut = self
            .shortcut
            .parse::<Shortcut>()
            .map_err(|error| format!("Invalid shortcut: {error}"))?;
        let file_shortcut =
            file_shortcut::canonical(&self.file_open_shortcut, cfg!(target_os = "macos"))
                .map_err(str::to_owned)?
                .parse::<Shortcut>()
                .map_err(|_| "fileShortcutInvalid".to_string())?;
        if file_shortcut == global_shortcut {
            return Err("fileShortcutConflict".into());
        }
        let mut ids = HashSet::new();
        let mut paths = HashSet::new();
        for scope in &self.scopes {
            if scope.id.is_empty() || !ids.insert(scope.id.clone()) {
                return Err("Duplicate or empty search folder ID.".into());
            }
            if scope.name.trim().is_empty() || !Path::new(&scope.path).is_absolute() {
                return Err("Search folders must have a name and an absolute path.".into());
            }
            let normalized = scope
                .path
                .replace('\\', "/")
                .trim_end_matches('/')
                .to_string();
            #[cfg(windows)]
            let normalized = normalized.to_lowercase();
            if !paths.insert(normalized) {
                return Err("This search folder has already been added.".into());
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SearchMode {
    #[default]
    Auto,
    Filename,
    Content,
    Semantic,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SearchSession {
    #[serde(default = "legacy_search_mode")]
    pub mode: SearchMode,
    pub query: String,
    pub scope_id: Option<String>,
    pub extension: String,
    pub selected_path: Option<String>,
    pub recent_queries: Vec<String>,
}
fn legacy_search_mode() -> SearchMode {
    SearchMode::Filename
}

pub fn read_settings(path: &Path) -> Result<ClientSettings, String> {
    match fs::read_to_string(path) {
        Ok(contents) => {
            let settings: ClientSettings = serde_json::from_str(&contents).map_err(|error| {
                format!(
                    "Could not read settings at {}: {error}. The file has not been changed.",
                    path.display()
                )
            })?;
            settings.validate()?;
            Ok(settings)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(ClientSettings::default()),
        Err(error) => Err(format!("Could not read settings: {error}")),
    }
}

pub use crate::settings_file::write_json;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn file_policy_migrates_legacy_settings_and_round_trips_without_granting_access() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("settings.json");
        fs::write(&file, r#"{"scopes":[],"filePolicy":{"filename":{"hidden":false,"archives":false},"content":{"sourceCode":false,"repositoryIgnored":false}}}"#).unwrap();
        let settings = read_settings(&file).unwrap();
        assert!(!settings.file_policy.filename.hidden);
        assert!(!settings.file_policy.filename.archives);
        assert!(settings.file_policy.filename.source_code);
        assert!(settings.file_policy.content.hidden);
        assert!(!settings.file_policy.content.source_code);
        assert!(!settings.file_policy.content.repository_ignored);
        assert!(settings.scopes.is_empty());
        assert!(settings.generation.local.is_none() && settings.generation.external.is_none());
        write_json(&file, &settings).unwrap();
        assert_eq!(
            read_settings(&file).unwrap().file_policy,
            settings.file_policy
        );
        fs::write(&file, "{}").unwrap();
        assert_eq!(
            read_settings(&file).unwrap().file_policy,
            crate::file_policy::Preferences::default()
        );
    }
    #[cfg(windows)]
    #[test]
    fn saved_scopes_keep_long_identity_and_permissions_after_an_alias_goes_offline() {
        let directory = tempfile::tempdir().unwrap();
        let folder = directory.path().join("Long configured folder name");
        fs::create_dir(&folder).unwrap();
        let Some(short) = crate::path_identity::short_alias(&folder).unwrap() else {
            eprintln!("No short aliases on fixture volume; actual alias case not exercised");
            return;
        };
        let canonical = crate::path_identity::normal(&fs::canonicalize(&folder).unwrap())
            .to_string_lossy()
            .into_owned();
        let mut settings = ClientSettings::default();
        settings.scopes.push(SearchScope {
            id: "stable".into(),
            name: "문서".into(),
            path: short.to_string_lossy().into_owned(),
            filename_enabled: false,
            content_enabled: true,
            external_allowed: false,
        });
        settings.normalize_scope_paths();
        settings.validate().unwrap();
        assert_eq!(settings.scopes[0].path, canonical);
        assert_eq!(settings.scopes[0].id, "stable");
        assert!(!settings.scopes[0].filename_enabled);
        assert!(settings.scopes[0].content_enabled);
        assert!(!settings.scopes[0].external_allowed);
        let file = directory.path().join("settings.json");
        write_json(&file, &settings).unwrap();
        fs::rename(&folder, directory.path().join("offline")).unwrap();
        assert_eq!(read_settings(&file).unwrap().scopes[0].path, canonical);
        fs::rename(directory.path().join("offline"), &folder).unwrap();
        let mut alias = settings.scopes[0].clone();
        alias.id = "duplicate".into();
        alias.path = short.to_string_lossy().into_owned();
        settings.scopes.push(alias);
        settings.normalize_scope_paths();
        assert!(settings.validate().is_err());
    }
    #[test]
    fn new_sessions_use_auto_but_legacy_and_explicit_modes_survive() {
        assert_eq!(
            serde_json::to_value(SearchSession::default()).unwrap()["mode"],
            "auto"
        );
        for (value, expected) in [
            (serde_json::json!({"query":"legacy"}), "filename"),
            (serde_json::json!({"mode":"auto"}), "auto"),
            (serde_json::json!({"mode":"semantic"}), "semantic"),
        ] {
            let session: SearchSession = serde_json::from_value(value).unwrap();
            assert_eq!(serde_json::to_value(session).unwrap()["mode"], expected);
        }
        let settings: ClientSettings = serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(settings.automatic_semantic);
    }

    #[test]
    fn only_new_profiles_require_onboarding_and_completion_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        assert_eq!(read_settings(&path).unwrap().onboarding_version, 0);
        fs::write(&path, r#"{"language":"ja","shortcut":"Ctrl+Shift+K"}"#).unwrap();
        let mut existing = read_settings(&path).unwrap();
        assert_eq!(existing.onboarding_version, 1);
        assert_eq!(existing.language, "ja");
        existing.onboarding_version = 0;
        write_json(&path, &existing).unwrap();
        assert_eq!(read_settings(&path).unwrap().onboarding_version, 0);
        existing.onboarding_version = 1;
        write_json(&path, &existing).unwrap();
        assert_eq!(read_settings(&path).unwrap().onboarding_version, 1);
    }

    #[test]
    fn settings_round_trip_preserves_korean_scopes_and_overwrites_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let mut settings = ClientSettings::default();
        settings.scopes.push(SearchScope {
            id: "docs".into(),
            name: "문서".into(),
            path: dir.path().to_string_lossy().into(),
            filename_enabled: true,
            content_enabled: false,
            external_allowed: false,
        });
        write_json(&path, &settings).unwrap();
        settings.theme = "dark".into();
        settings.hybrid_search = false;
        settings.generation.retrieval = crate::rag_service::RetrievalMode::Keyword;
        settings.automatic_semantic = false;
        settings.revision = 1;
        write_json(&path, &settings).unwrap();
        let loaded = read_settings(&path).unwrap();
        assert_eq!(loaded.theme, "dark");
        assert!(!loaded.hybrid_search);
        assert_eq!(
            loaded.generation.retrieval,
            crate::rag_service::RetrievalMode::Keyword
        );
        assert!(!loaded.automatic_semantic);
        assert_eq!(loaded.revision, 1);
        assert_eq!(loaded.scopes[0].name, "문서");
        assert!(!loaded.scopes[0].content_enabled);
    }

    #[test]
    fn corrupt_settings_are_reported_and_never_replaced_by_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(&path, "{broken").unwrap();
        assert!(read_settings(&path).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "{broken");
    }

    #[test]
    fn rejects_relative_paths_invalid_shortcuts_and_future_schema() {
        let mut settings = ClientSettings::default();
        settings.shortcut = "A".into();
        assert!(settings.validate().is_err());
        settings.shortcut = "Ctrl+Shift+Space".into();
        assert!(settings.validate().is_ok());
        settings.schema_version = 2;
        assert!(settings.validate().is_err());
        settings.schema_version = 1;
        settings.scopes.push(SearchScope {
            id: "bad".into(),
            name: "relative".into(),
            path: "../Documents".into(),
            filename_enabled: true,
            content_enabled: false,
            external_allowed: false,
        });
        assert!(settings.validate().is_err());
    }

    #[test]
    fn file_opening_key_migrates_round_trips_and_rejects_global_conflicts() {
        let mut settings: ClientSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings.file_open_shortcut, "Enter");
        settings.file_open_shortcut = "Control+Shift+O".into();
        settings.validate().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        write_json(&path, &settings).unwrap();
        assert_eq!(
            read_settings(&path).unwrap().file_open_shortcut,
            "Control+Shift+O"
        );
        settings.shortcut = "Shift+Ctrl+KeyO".into();
        assert_eq!(settings.validate(), Err("fileShortcutConflict".into()));
        settings.file_open_shortcut = "Ctrl+Enter".into();
        assert_eq!(settings.validate(), Err("fileShortcutReserved".into()));
    }

    #[test]
    fn mcp_servers_are_tidied_then_strictly_validated() {
        let server = |id: &str, command: &str| crate::mcp_client::ServerConfig {
            id: id.into(),
            name: String::new(),
            command: command.into(),
            args: vec!["--stdio".into(), String::new()],
            allowed_tools: vec![],
            search: None,
        };
        let mut mcp = McpPreferences {
            enabled: true,
            servers: vec![server("a-1", "C:/Tools/wiki-mcp.exe"), server("b-2", "  ")],
        };
        mcp.tidy();
        assert_eq!(mcp.servers.len(), 1, "the empty new row is dropped");
        assert_eq!(mcp.servers[0].name, "wiki-mcp");
        assert_eq!(mcp.servers[0].args, ["--stdio"]);
        assert!(mcp.validate().is_ok());

        // Ids must be unique and simple; programs/arguments cannot carry NUL; at most 8.
        let mut duplicate = mcp.clone();
        duplicate.servers.push(duplicate.servers[0].clone());
        assert!(duplicate.validate().is_err());
        let mut bad_id = mcp.clone();
        bad_id.servers[0].id = "../x".into();
        assert!(bad_id.validate().is_err());
        let mut nul = mcp.clone();
        nul.servers[0].args = vec!["a\0b".into()];
        assert!(nul.validate().is_err());
        let mut many = mcp.clone();
        many.servers = (0..9).map(|i| server(&format!("s{i}"), "x")).collect();
        many.tidy();
        assert!(many.validate().is_err());
    }
}
