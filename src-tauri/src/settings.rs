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
    pub theme: String,
    pub language: String,
    pub font_size: u8,
    pub shortcut: String,
    pub remember_search: bool,
    pub keyboard_correction: bool,
    pub close_to_tray: bool,
    pub start_on_login: bool,
    pub scopes: Vec<SearchScope>,
    pub calculator: CalculatorSettings,
    pub clipboard: crate::clipboard_history::ClipboardSettings,
    pub generation: crate::rag_service::GenerationPreferences,
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
            theme: "system".into(),
            language: "ko".into(),
            font_size: 14,
            shortcut: "Alt+Space".into(),
            remember_search: true,
            keyboard_correction: true,
            close_to_tray: true,
            start_on_login: true,
            scopes: vec![],
            calculator: CalculatorSettings::default(),
            clipboard: crate::clipboard_history::ClipboardSettings::default(),
            generation: crate::rag_service::GenerationPreferences::default(),
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
    Filename,
    Content,
    Semantic,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SearchSession {
    pub mode: SearchMode,
    pub query: String,
    pub scope_id: Option<String>,
    pub extension: String,
    pub selected_path: Option<String>,
    pub recent_queries: Vec<String>,
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
        settings.revision = 1;
        write_json(&path, &settings).unwrap();
        let loaded = read_settings(&path).unwrap();
        assert_eq!(loaded.theme, "dark");
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
