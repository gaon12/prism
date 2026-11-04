mod commands;
mod filename_pattern;
mod index;
mod settings;
mod windows;

use settings::{ClientSettings, SearchSession};
use std::{fs, path::PathBuf, sync::Mutex};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager, WindowEvent,
};
use tauri_plugin_autostart::ManagerExt as AutostartExt;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

pub struct AppState {
    settings: Mutex<ClientSettings>,
    session: Mutex<SearchSession>,
    shortcut_error: Mutex<Option<String>>,
    startup_error: Mutex<Option<String>>,
    config_error: Mutex<Option<String>>,
    data_dir: PathBuf,
    index: index::FileIndex,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();
    // Isolated debug tests must not hand their launch to an already running user instance.
    let isolated_test = cfg!(debug_assertions) && std::env::var_os("PRISM_DEV_DATA_DIR").is_some();
    let builder = if isolated_test {
        builder
    } else {
        builder.plugin(tauri_plugin_single_instance::init(|app, _, _| {
            let _ = windows::show_main_window(app);
        }))
    };
    builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(
            tauri_plugin_autostart::Builder::new()
                .arg("--background")
                .build(),
        )
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _, event| {
                    if event.state == ShortcutState::Pressed {
                        if let Err(error) = windows::toggle_quick(app) {
                            eprintln!("Quick-search shortcut failed: {error}");
                        }
                    }
                })
                .build(),
        )
        .setup(|app| {
            let data_dir = app.path().app_config_dir()?;
            #[cfg(debug_assertions)]
            let data_dir = std::env::var_os("PRISM_DEV_DATA_DIR")
                .map(PathBuf::from)
                .unwrap_or(data_dir);
            fs::create_dir_all(&data_dir)?;
            let (settings, config_error) =
                match settings::read_settings(&data_dir.join("settings.json")) {
                    Ok(settings) => (settings, None),
                    Err(error) => (ClientSettings::default(), Some(error)),
                };
            let session = if settings.remember_search {
                fs::read_to_string(data_dir.join("session.json"))
                    .ok()
                    .and_then(|contents| serde_json::from_str(&contents).ok())
                    .unwrap_or_default()
            } else {
                SearchSession::default()
            };
            let shortcut_error = if config_error.is_none() {
                app.global_shortcut()
                    .register(settings.shortcut.as_str())
                    .err()
                    .map(|error| error.to_string())
            } else {
                None
            };
            let startup_error = if !cfg!(debug_assertions) && config_error.is_none() {
                (if settings.start_on_login {
                    app.autolaunch().enable()
                } else {
                    app.autolaunch().disable()
                })
                .err()
                .map(|error| error.to_string())
            } else {
                None
            };
            let index = index::FileIndex::new();
            if config_error.is_none() {
                index.configure(settings.scopes.clone());
            }
            app.manage(AppState {
                settings: Mutex::new(settings),
                session: Mutex::new(session),
                shortcut_error: Mutex::new(shortcut_error),
                startup_error: Mutex::new(startup_error),
                config_error: Mutex::new(config_error),
                data_dir,
                index,
            });
            windows::build_quick(app.handle())?;
            let open = MenuItem::with_id(app, "open", "Prism 열기", true, None::<&str>)?;
            let quick = MenuItem::with_id(app, "quick", "빠른 검색", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "종료", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &quick, &quit])?;
            let mut tray = TrayIconBuilder::with_id("prism")
                .tooltip("Prism")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => {
                        let _ = windows::show_main_window(app);
                    }
                    "quick" => {
                        let _ = windows::show_quick_window(app);
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let _ = windows::show_main_window(tray.app_handle());
                    }
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;
            if std::env::args().any(|argument| argument == "--background") {
                if let Some(window) = app.get_webview_window("main") {
                    window.hide()?;
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| match event {
            WindowEvent::CloseRequested { api, .. } => {
                if window.label() == "quick" {
                    api.prevent_close();
                    let _ = window.hide();
                } else if let Some(state) = window.app_handle().try_state::<AppState>() {
                    if state
                        .settings
                        .lock()
                        .map(|settings| settings.close_to_tray)
                        .unwrap_or(false)
                    {
                        api.prevent_close();
                        let _ = window.hide();
                    } else {
                        window.app_handle().exit(0);
                    }
                }
            }
            WindowEvent::Focused(false) if window.label() == "quick" => {
                let _ = window.hide();
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            commands::save_settings,
            commands::get_search_session,
            commands::set_search_session,
            commands::recommended_folders,
            commands::runtime_info,
            commands::show_quick,
            commands::show_main,
            commands::hide_quick,
            commands::resize_quick,
            commands::index_status,
            commands::search_files,
            commands::refresh_index,
            commands::file_action,
            commands::open_web_search,
            commands::copy_text,
            commands::window_action,
            commands::quit_app,
            commands::write_markdown
        ])
        .run(tauri::generate_context!())
        .expect("Prism could not start");
}
