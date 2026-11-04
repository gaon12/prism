use crate::{
    settings::{write_json, ClientSettings, SearchSession},
    windows, AppState,
};
use serde::Serialize;
use std::{fs, path::PathBuf};
use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow};
use tauri_plugin_autostart::ManagerExt as AutostartExt;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};

#[tauri::command]
pub fn get_settings(app: AppHandle, state: State<'_, AppState>) -> Result<ClientSettings, String> {
    let mut config_error = state
        .config_error
        .lock()
        .map_err(|_| "Settings are unavailable.")?;
    if config_error.is_some() {
        let restored = crate::settings::read_settings(&state.data_dir.join("settings.json"))?;
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
            .map_err(|_| "Settings are unavailable.")? = restored;
        *config_error = None;
    }
    Ok(state
        .settings
        .lock()
        .map_err(|_| "Settings are unavailable.")?
        .clone())
}

fn set_autostart(app: &AppHandle, enabled: bool) -> Result<(), String> {
    if cfg!(debug_assertions) {
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
    settings.validate()?;
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
    *current = settings.clone();
    if let Some(quick) = app.get_webview_window("quick") {
        let extra = f64::from(settings.font_size.saturating_sub(14));
        let _ = quick.set_size(tauri::LogicalSize::new(
            720.0 + extra * 20.0,
            480.0 + extra * 24.0,
        ));
    }
    *state
        .shortcut_error
        .lock()
        .map_err(|_| "Shortcut state is unavailable.")? = None;
    drop(current);
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
    native: bool,
    development: bool,
    shortcut_error: Option<String>,
    startup_error: Option<String>,
}

#[tauri::command]
pub fn runtime_info(state: State<'_, AppState>) -> Result<RuntimeInfo, String> {
    Ok(RuntimeInfo {
        native: true,
        development: cfg!(debug_assertions),
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
