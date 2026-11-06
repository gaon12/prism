use crate::{
    settings::{write_json, ClientSettings, SearchSession},
    windows, AppState,
};
use serde::Serialize;
use std::{fs, path::PathBuf};
use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow};
use tauri_plugin_autostart::ManagerExt as AutostartExt;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};
use tauri_plugin_opener::OpenerExt;

#[tauri::command]
pub fn copy_text(state: State<'_, AppState>, text: String) -> Result<(), String> {
    if text.len() > 1_048_576 {
        return Err("Text is too long to copy.".into());
    }
    state
        .clipboard
        .write_content(crate::clipboard_history::ClipContent::Text(text))
}

#[tauri::command]
pub fn get_settings(app: AppHandle, state: State<'_, AppState>) -> Result<ClientSettings, String> {
    let mut config_error = state
        .config_error
        .lock()
        .map_err(|_| "Settings are unavailable.")?;
    if config_error.is_some() {
        let mut restored = crate::settings::read_settings(&state.data_dir.join("settings.json"))?;
        if state.location.kind == crate::app_paths::InstallKind::Portable {
            restored.start_on_login = false;
        }
        let error = app
            .global_shortcut()
            .register(restored.shortcut.as_str())
            .err()
            .map(|error| error.to_string());
        *state
            .shortcut_error
            .lock()
            .map_err(|_| "Shortcut state is unavailable.")? = error;
        *state
            .startup_error
            .lock()
            .map_err(|_| "Startup state is unavailable.")? =
            set_autostart(&app, restored.start_on_login).err();
        *state
            .settings
            .lock()
            .map_err(|_| "Settings are unavailable.")? = restored.clone();
        if let Ok(content) = &state.content {
            content.set_battery_pause(restored.power.pause_on_battery);
            let _ = content.configure_with_ocr(restored.scopes.clone(), restored.ocr.clone());
        }
        state.index.configure(restored.scopes);
        state.clipboard.apply_settings(restored.clipboard)?;
        *config_error = None;
    }
    Ok(state
        .settings
        .lock()
        .map_err(|_| "Settings are unavailable.")?
        .clone())
}

fn set_autostart(app: &AppHandle, enabled: bool) -> Result<(), String> {
    apply_autostart(app, &app.state::<AppState>().location, enabled)
}

pub(crate) fn apply_autostart(
    app: &AppHandle,
    location: &crate::app_paths::DataLocation,
    enabled: bool,
) -> Result<(), String> {
    // Debug fixtures must never change the real user's login configuration.
    if cfg!(debug_assertions) {
        return Ok(());
    }
    if location.kind == crate::app_paths::InstallKind::Packaged {
        return crate::packaged_startup::set(enabled);
    }
    if !location.allows_autostart() {
        return Ok(());
    }
    if enabled {
        app.autolaunch().enable()
    } else {
        app.autolaunch().disable()
    }
    .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn save_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    mut settings: ClientSettings,
    expected_revision: u64,
) -> Result<ClientSettings, String> {
    if let Some(error) = state
        .config_error
        .lock()
        .map_err(|_| "Settings are unavailable.")?
        .clone()
    {
        return Err(error);
    }
    settings.mcp.tidy();
    settings.validate()?;
    settings.normalize_scope_paths();
    settings.validate()?;
    if state.location.kind == crate::app_paths::InstallKind::Portable && settings.start_on_login {
        return Err("Login startup is unavailable in portable mode.".into());
    }
    let mut current = state
        .settings
        .lock()
        .map_err(|_| "Settings are unavailable.")?;
    if current.revision != expected_revision {
        return Err("Settings changed in another window. Reopen settings and try again.".into());
    }
    let old_shortcut: Shortcut = current
        .shortcut
        .parse()
        .map_err(|error| format!("Invalid previous shortcut: {error}"))?;
    let new_shortcut: Shortcut = settings
        .shortcut
        .parse()
        .map_err(|error| format!("Invalid shortcut: {error}"))?;
    let changing_shortcut = old_shortcut != new_shortcut;
    let new_registration = changing_shortcut || !app.global_shortcut().is_registered(new_shortcut);
    if new_registration {
        app.global_shortcut()
            .register(new_shortcut)
            .map_err(|error| format!("Shortcut unavailable: {error}"))?;
    }
    if settings.start_on_login != current.start_on_login {
        if let Err(error) = set_autostart(&app, settings.start_on_login) {
            if new_registration {
                let _ = app.global_shortcut().unregister(new_shortcut);
            }
            return Err(error);
        }
    }
    settings.revision = current
        .revision
        .checked_add(1)
        .ok_or("Settings revision overflow.")?;
    if let Err(error) = write_json(&state.data_dir.join("settings.json"), &settings) {
        if new_registration {
            let _ = app.global_shortcut().unregister(new_shortcut);
        }
        if settings.start_on_login != current.start_on_login {
            let _ = set_autostart(&app, current.start_on_login);
        }
        return Err(error);
    }
    if changing_shortcut {
        let _ = app.global_shortcut().unregister(old_shortcut);
    }
    let changed_scopes = current
        .scopes
        .iter()
        .map(|scope| (&scope.id, &scope.path, scope.filename_enabled))
        .collect::<Vec<_>>()
        != settings
            .scopes
            .iter()
            .map(|scope| (&scope.id, &scope.path, scope.filename_enabled))
            .collect::<Vec<_>>();
    let changed_content = current.ocr != settings.ocr
        || current
            .scopes
            .iter()
            .map(|scope| (&scope.id, &scope.path, scope.content_enabled))
            .collect::<Vec<_>>()
            != settings
                .scopes
                .iter()
                .map(|scope| (&scope.id, &scope.path, scope.content_enabled))
                .collect::<Vec<_>>();
    *current = settings.clone();
    if let Ok(updates) = &state.updates {
        updates.set_auto_check(settings.updates.auto_check);
    }
    if let Ok(documents) = &state.documents {
        documents.cancel_all();
    }
    // Cancel in-flight generation after committing the new settings. The RAG
    // service independently rechecks the latest policy before every provider call.
    if let Ok(rag) = &state.rag {
        rag.cancel_all();
    }
    // The battery setting is cheap to apply and may change on its own, so always apply it.
    if let Ok(content) = &state.content {
        content.set_battery_pause(settings.power.pause_on_battery);
    }
    if changed_content {
        if let Ok(content) = &state.content {
            let _ = content.configure_with_ocr(settings.scopes.clone(), settings.ocr.clone());
        }
    }
    // The persisted settings are authoritative. Runtime storage errors are reported by
    // clipboard_status; do not report a failed settings save after its atomic commit.
    let _ = state.clipboard.apply_settings(settings.clipboard.clone());
    if changed_scopes {
        state.index.configure(settings.scopes.clone());
    }
    *state
        .shortcut_error
        .lock()
        .map_err(|_| "Shortcut state is unavailable.")? = None;
    drop(current);
    let expanded = state
        .session
        .lock()
        .map(|session| !session.query.trim().is_empty())
        .unwrap_or(false);
    let _ = windows::resize_quick(&app, expanded);
    if !settings.remember_search {
        let mut session = state
            .session
            .lock()
            .map_err(|_| "Search state is unavailable.")?;
        session.recent_queries.clear();
        write_json(
            &state.data_dir.join("session.json"),
            &SearchSession::default(),
        )?;
        let _ = app.emit("prism:session", &*session);
    }
    let _ = app.emit("prism:settings", &settings);
    Ok(settings)
}

#[tauri::command]
pub fn resize_quick(app: AppHandle, expanded: bool) -> Result<(), String> {
    windows::resize_quick(&app, expanded)
}

#[tauri::command]
pub fn index_status(state: State<'_, AppState>) -> Result<crate::index::IndexStatus, String> {
    state.index.status()
}

#[tauri::command]
pub async fn search_files(
    state: State<'_, AppState>,
    query: String,
    extension: String,
    scope_id: Option<String>,
) -> Result<crate::index::SearchResponse, String> {
    let index = state.index.clone();
    let allow_keyboard = state
        .settings
        .lock()
        .map_err(|_| "Settings unavailable.")?
        .keyboard_correction;
    tauri::async_runtime::spawn_blocking(move || {
        index.search_with_keyboard(&query, &extension, scope_id.as_deref(), 200, allow_keyboard)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub fn refresh_index(state: State<'_, AppState>) -> Result<(), String> {
    let scopes = state
        .settings
        .lock()
        .map_err(|_| "Settings unavailable.")?
        .scopes
        .clone();
    state.index.refresh(scopes);
    Ok(())
}

#[tauri::command]
pub async fn file_action(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
    reveal: bool,
) -> Result<(), String> {
    let path = state.index.resolve_result(&path)?;
    tauri::async_runtime::spawn_blocking(move || {
        if reveal {
            app.opener().reveal_item_in_dir(path)
        } else {
            app.opener()
                .open_path(path.to_string_lossy().to_string(), None::<&str>)
        }
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn open_web_search(
    app: AppHandle,
    provider: String,
    query: String,
) -> Result<(), String> {
    if query.trim().is_empty() || query.len() > 4096 {
        return Err("Enter a search term (4096 bytes maximum).".into());
    }
    // Only built-in HTTPS search providers are allowed; never execute user input as a URL or shell command.
    let base = match provider.as_str() {
        "google" => "https://www.google.com/search?q=",
        "bing" => "https://www.bing.com/search?q=",
        "ddg" => "https://duckduckgo.com/?q=",
        "namu" => "https://namu.wiki/Search?q=",
        "wiki" => "https://en.wikipedia.org/w/index.php?search=",
        _ => return Err("Unknown web search provider.".into()),
    };
    let mut url = tauri::Url::parse(base).map_err(|error| error.to_string())?;
    let key = if provider == "wiki" { "search" } else { "q" };
    url.query_pairs_mut().clear().append_pair(key, query.trim());
    tauri::async_runtime::spawn_blocking(move || {
        app.opener()
            .open_url(url.as_str(), None::<&str>)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub fn get_search_session(state: State<'_, AppState>) -> Result<SearchSession, String> {
    Ok(state
        .session
        .lock()
        .map_err(|_| "Search state is unavailable.")?
        .clone())
}

#[tauri::command]
pub fn set_search_session(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    mut session: SearchSession,
) -> Result<(), String> {
    if session.query.len() > 32768 || session.extension.len() > 1024 {
        return Err("Search input is too long.".into());
    }
    session.recent_queries.truncate(20);
    let remember = state
        .settings
        .lock()
        .map_err(|_| "Settings are unavailable.")?
        .remember_search;
    if !remember {
        session.recent_queries.clear();
    }
    let mut current = state
        .session
        .lock()
        .map_err(|_| "Search state is unavailable.")?;
    if remember {
        write_json(&state.data_dir.join("session.json"), &session)?;
    }
    *current = session;
    // Do not echo an older debounced write into the window that is still typing.
    for (label, other) in app.webview_windows() {
        if label != window.label() {
            let _ = other.emit("prism:session", &*current);
        }
    }
    Ok(())
}

#[derive(Serialize)]
pub struct RecommendedFolder {
    name: String,
    path: String,
}

#[tauri::command]
pub fn recommended_folders(app: AppHandle) -> Vec<RecommendedFolder> {
    let paths = [
        app.path().document_dir(),
        app.path().download_dir(),
        app.path().desktop_dir(),
    ];
    paths
        .into_iter()
        .filter_map(Result::ok)
        .filter(|path| path.is_dir())
        .map(|path| RecommendedFolder {
            name: path
                .file_name()
                .map(|name| name.to_string_lossy().into())
                .unwrap_or_else(|| path.display().to_string()),
            path: path.to_string_lossy().into(),
        })
        .collect()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeInfo {
    platform: &'static str,
    native: bool,
    development: bool,
    install_kind: crate::app_paths::InstallKind,
    shortcut_error: Option<String>,
    startup_error: Option<String>,
}

#[tauri::command]
pub fn runtime_info(state: State<'_, AppState>) -> Result<RuntimeInfo, String> {
    Ok(RuntimeInfo {
        platform: std::env::consts::OS,
        native: true,
        development: cfg!(debug_assertions),
        install_kind: state.location.kind,
        startup_error: state
            .startup_error
            .lock()
            .map_err(|_| "Startup state is unavailable.")?
            .clone(),
        shortcut_error: state
            .shortcut_error
            .lock()
            .map_err(|_| "Shortcut state is unavailable.")?
            .clone(),
    })
}

#[tauri::command]
pub fn show_quick(app: AppHandle) -> Result<(), String> {
    windows::show_quick_window(&app)
}
#[tauri::command]
pub fn show_main(app: AppHandle) -> Result<(), String> {
    windows::show_main_window(&app)
}
#[tauri::command]
pub fn open_settings(app: AppHandle, tab: String) -> Result<(), String> {
    if !["general", "scopes", "shortcuts", "ai"].contains(&tab.as_str()) {
        return Err("Unknown settings page.".into());
    }
    windows::show_main_window(&app)?;
    app.emit_to("main", "prism:open-settings", tab)
        .map_err(|_| "Could not open settings.".into())
}
#[tauri::command]
pub fn hide_quick(app: AppHandle) -> Result<(), String> {
    app.get_webview_window("quick")
        .ok_or("Quick window is unavailable.")?
        .hide()
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn window_action(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    action: &str,
) -> Result<(), String> {
    let result = match action {
        "minimize" => window.minimize(),
        "maximize" => {
            if window.is_maximized().map_err(|error| error.to_string())? {
                window.unmaximize()
            } else {
                window.maximize()
            }
        }
        "close" => {
            if state
                .settings
                .lock()
                .map_err(|_| "Settings are unavailable.")?
                .close_to_tray
            {
                window.hide()
            } else {
                app.exit(0);
                Ok(())
            }
        }
        _ => return Err("Unknown window action.".into()),
    };
    result.map_err(|error| error.to_string())
}

#[tauri::command]
pub fn quit_app(app: AppHandle) {
    app.exit(0);
}

#[tauri::command]
pub async fn write_markdown(path: String, contents: String) -> Result<(), String> {
    let path = PathBuf::from(path);
    if !path.is_absolute()
        || path
            .extension()
            .and_then(|value| value.to_str())
            .map(|value| !value.eq_ignore_ascii_case("md"))
            .unwrap_or(true)
    {
        return Err("Choose an absolute Markdown (.md) file path.".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        fs::write(path, contents).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}
