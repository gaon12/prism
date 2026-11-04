use serde::{Deserialize, Serialize};
use std::{collections::HashSet, fs, io::Write, path::Path};
use tauri_plugin_global_shortcut::Shortcut;

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
    pub remember_search: bool,
    pub keyboard_correction: bool,
    pub hybrid_search: bool,
    pub automatic_semantic: bool,
    pub close_to_tray: bool,
    pub start_on_login: bool,
    pub scopes: Vec<SearchScope>,
    pub calculator: CalculatorSettings,
    pub clipboard: crate::clipboard_history::ClipboardSettings,
    pub generation: crate::rag_service::GenerationPreferences,
    pub updates: UpdatePreferences,
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
            remember_search: true,
            keyboard_correction: true,
            hybrid_search: true,
            automatic_semantic: true,
            close_to_tray: true,
            start_on_login: true,
            scopes: vec![],
            calculator: CalculatorSettings::default(),
            clipboard: crate::clipboard_history::ClipboardSettings::default(),
            generation: crate::rag_service::GenerationPreferences::default(),
            updates: UpdatePreferences::default(),
        }
    }
}

impl ClientSettings {
    pub fn validate(&self) -> Result<(), String> {
        self.clipboard.validate()?;
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
        self.shortcut
            .parse::<Shortcut>()
            .map_err(|error| format!("Invalid shortcut: {error}"))?;
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

pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let contents = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    let temporary = path.with_extension("pending");
    let parent = path.parent().ok_or("No parent directory for settings.")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let mut file = fs::File::create(&temporary).map_err(|error| error.to_string())?;
    file.write_all(&contents)
        .and_then(|_| file.sync_all())
        .map_err(|error| error.to_string())?;
    drop(file);
    fs::rename(&temporary, path)
        .map_err(|error| format!("Could not save {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
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
        settings.automatic_semantic = false;
        settings.revision = 1;
        write_json(&path, &settings).unwrap();
        let loaded = read_settings(&path).unwrap();
        assert_eq!(loaded.theme, "dark");
        assert!(!loaded.hybrid_search);
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
}
