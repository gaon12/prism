mod clipboard_commands;
mod clipboard_history;
mod commands;
mod content_commands;
mod content_service;
mod content_store;
mod dictionary_commands;
mod dictionary_provider;
mod dictionary_service;
mod dictionary_store;
mod document_extract;
mod embedding_runtime;
mod filename_pattern;
mod generation_runtime;
mod index;
mod keyboard_pattern;
mod model_commands;
mod process_job;
mod rag_commands;
mod rag_service;
mod rag_sources;
mod rag_store;
mod secret_store;
mod settings;
mod vector_store;
mod windows;

use settings::{ClientSettings, SearchSession};
use std::{fs, path::PathBuf, sync::Mutex};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager, WindowEvent,
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
    clipboard: clipboard_history::ClipboardService,
    dictionary: Result<dictionary_service::DictionaryService, dictionary_service::DictionaryError>,
    embedding: Result<embedding_runtime::EmbeddingRuntime, String>,
    content: Result<content_service::ContentService, content_service::ContentError>,
    rag: Result<rag_service::RagService, rag_service::RagError>,
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
        .setup(move |app| {
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
            let mut clipboard_settings = settings.clipboard.clone();
            if config_error.is_some() {
                clipboard_settings.enabled = false;
                // Preserve history until the user repairs the corrupt settings file.
                clipboard_settings.max_age_days = 0;
                clipboard_settings.max_items = 10_000;
                clipboard_settings.max_bytes = 8u64 << 30;
            }
            let clipboard = clipboard_history::ClipboardService::open(
                data_dir.join("clipboard"),
                clipboard_settings,
                {
                    let app = app.handle().clone();
                    move |status| {
                        let _ = app.emit("prism:clipboard", status);
                    }
                },
            );
            if isolated_test {
                let source = clipboard_history::MemoryClipboard::default();
                clipboard.start(source.boxed(), source.boxed());
            } else {
                clipboard.start(
                    clipboard_history::system_source(app.handle().clone()),
                    clipboard_history::system_source(app.handle().clone()),
                );
            }
            // Keep large local caches out of the roaming settings profile. Tests always use
            // their explicit isolated directory for every service and model asset.
            let asset_dir = if isolated_test {
                data_dir.clone()
            } else {
                app.path().app_local_data_dir()?
            };
            let embedding = embedding_runtime::EmbeddingRuntime::new(asset_dir.clone());
            let content = content_service::ContentService::open(
                &asset_dir.join("content"),
                embedding.clone(),
                {
                    let app = app.handle().clone();
                    move |status| {
                        let _ = app.emit("prism:content", status);
                    }
                },
            );
            if let Ok(content) = &content {
                content.exclude_paths(&[
                    asset_dir.join("embedding"),
                    asset_dir.join("dictionary"),
                    data_dir.join("clipboard"),
                    data_dir.join("settings.json"),
                    data_dir.join("session.json"),
                    data_dir.join("chats"),
                    data_dir.join("credentials"),
                    asset_dir.join("credentials"),
                    asset_dir.join("chats"),
                    asset_dir.join("rag"),
                    asset_dir.join("generation-secrets"),
                ]);
            }
            if config_error.is_none() {
                if let Ok(content) = &content {
                    let _ = content.configure(settings.scopes.clone());
                }
            }
            let dictionary =
                dictionary_service::DictionaryService::open(&asset_dir.join("dictionary"));
            let rag = content
                .as_ref()
                .map_err(|_| rag_service::RagError::new(rag_service::RagErrorCode::Storage))
                .and_then(|content| {
                    let handle = app.handle().clone();
                    rag_service::RagService::open(
                        &asset_dir,
                        std::sync::Arc::new(move || {
                            let state = handle.try_state::<AppState>().ok_or_else(|| {
                                rag_service::RagError::new(rag_service::RagErrorCode::Worker)
                            })?;
                            if state
                                .config_error
                                .lock()
                                .map_err(|_| {
                                    rag_service::RagError::new(rag_service::RagErrorCode::Worker)
                                })?
                                .is_some()
                            {
                                return Err(rag_service::RagError::new(
                                    rag_service::RagErrorCode::Storage,
                                ));
                            }
                            let settings = state.settings.lock().map_err(|_| {
                                rag_service::RagError::new(rag_service::RagErrorCode::Worker)
                            })?;
                            Ok(rag_service::RagPolicy {
                                revision: settings.revision,
                                prefs: settings.generation.clone(),
                                scopes: settings
                                    .scopes
                                    .iter()
                                    .map(|scope| rag_service::RagScope {
                                        id: scope.id.clone(),
                                        name: scope.name.clone(),
                                        path: scope.path.clone(),
                                        content_enabled: scope.content_enabled,
                                        external_allowed: scope.external_allowed,
                                    })
                                    .collect(),
                            })
                        }),
                        std::sync::Arc::new(rag_sources::ContentSources(content.clone())),
                        secret_store::SecretStore::new(asset_dir.clone()),
                    )
                });
            app.manage(AppState {
                settings: Mutex::new(settings),
                session: Mutex::new(session),
                shortcut_error: Mutex::new(shortcut_error),
                startup_error: Mutex::new(startup_error),
                config_error: Mutex::new(config_error),
                data_dir,
                index,
                clipboard,
                dictionary,
                embedding,
                content,
                rag,
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
            commands::open_settings,
            commands::hide_quick,
            commands::resize_quick,
            commands::index_status,
            commands::search_files,
            commands::refresh_index,
            commands::file_action,
            commands::open_web_search,
            commands::copy_text,
            clipboard_commands::clipboard_list,
            clipboard_commands::clipboard_status,
            clipboard_commands::clipboard_text,
            clipboard_commands::clipboard_thumbnail,
            clipboard_commands::clipboard_copy,
            clipboard_commands::clipboard_pin,
            clipboard_commands::clipboard_delete,
            clipboard_commands::clipboard_clear,
            clipboard_commands::clipboard_set_paused,
            dictionary_commands::dictionary_lookup,
            dictionary_commands::dictionary_packs,
            dictionary_commands::dictionary_job,
            dictionary_commands::dictionary_cancel,
            dictionary_commands::dictionary_import,
            dictionary_commands::dictionary_download,
            dictionary_commands::dictionary_remove,
            dictionary_commands::dictionary_open_source,
            model_commands::model_status,
            model_commands::model_cancel,
            model_commands::model_download,
            model_commands::model_import,
            model_commands::model_start,
            model_commands::model_stop,
            content_commands::content_status,
            content_commands::content_refresh,
            content_commands::content_cancel_index,
            content_commands::content_cancel_search,
            content_commands::search_content,
            content_commands::search_semantic,
            content_commands::content_chunks,
            content_commands::content_file_action,
            rag_commands::rag_create,
            rag_commands::rag_list,
            rag_commands::rag_rename,
            rag_commands::rag_delete,
            rag_commands::rag_conversation,
            rag_commands::rag_ask,
            rag_commands::rag_cancel,
            rag_commands::rag_job,
            rag_commands::rag_export,
            rag_commands::rag_sources_hidden,
            rag_commands::generation_credential_status,
            rag_commands::generation_set_key,
            rag_commands::generation_clear_key,
            commands::window_action,
            commands::quit_app,
            commands::write_markdown
        ])
        .build(tauri::generate_context!())
        .expect("Prism could not start")
        .run(|app, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                if let Some(state) = app.try_state::<AppState>() {
                    if let Ok(rag) = &state.rag {
                        rag.cancel_all();
                    }
                    if let Ok(content) = &state.content {
                        content.shutdown();
                    }
                    if let Ok(embedding) = &state.embedding {
                        let _ = embedding.stop();
                    }
                }
            }
        });
}
