mod app_paths;
mod automatic_search;
mod chat_selection;
mod clipboard_commands;
mod clipboard_history;
mod commands;
mod content_commands;
mod content_service;
mod content_store;
mod diagnostics;
mod dictionary_commands;
mod dictionary_provider;
mod dictionary_service;
mod dictionary_store;
mod document_commands;
mod document_extract;
mod document_service;
mod document_store;
mod embedding_assets;
mod embedding_package;
mod embedding_runtime;
mod filename_pattern;
mod foreground_tracker;
mod generation_runtime;
mod hybrid_search;
mod index;
mod keyboard_pattern;
mod model_commands;
mod ocr;
mod package_context;
mod packaged_startup;
#[cfg(debug_assertions)]
mod paste_fixture;
mod paste_target;
mod path_identity;
mod process_job;
mod rag_commands;
mod rag_service;
mod rag_sources;
mod rag_store;
mod runtime_process;
mod runtime_supervisor;
mod secret_store;
mod selection_set;
mod settings;
mod update_backend;
mod update_commands;
#[cfg(debug_assertions)]
mod update_fixture;
mod update_notice;
mod update_runtime;
mod update_service;
mod update_state;
mod vector_store;
mod windows;

/// Internal Unix helper mode. Dispatch before desktop initialization.
pub fn run_runtime_supervisor() -> Option<i32> {
    runtime_supervisor::dispatch()
}

use settings::{ClientSettings, SearchSession};
use std::{fs, path::PathBuf, sync::Mutex};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager, WindowEvent,
};
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

pub struct AppState {
    settings: Mutex<ClientSettings>,
    session: Mutex<SearchSession>,
    shortcut_error: Mutex<Option<String>>,
    startup_error: Mutex<Option<String>>,
    config_error: Mutex<Option<String>>,
    chat_handoff: Mutex<Option<String>>,
    data_dir: PathBuf,
    location: app_paths::DataLocation,
    index: index::FileIndex,
    clipboard: clipboard_history::ClipboardService,
    paste_targets: std::sync::Arc<paste_target::PasteTargets>,
    paste_input: std::sync::Arc<dyn paste_target::InputBackend + Send + Sync>,
    #[cfg(debug_assertions)]
    input_fixture: Option<std::sync::Arc<paste_fixture::MemoryInput>>,
    #[cfg(debug_assertions)]
    clipboard_fixture: Option<clipboard_history::MemoryClipboard>,
    dictionary: Result<dictionary_service::DictionaryService, dictionary_service::DictionaryError>,
    embedding: Result<embedding_runtime::EmbeddingRuntime, String>,
    content: Result<content_service::ContentService, content_service::ContentError>,
    rag: Result<rag_service::RagService, rag_service::RagError>,
    documents: Result<document_service::DocumentService, document_service::DocumentError>,
    updates: Result<update_runtime::UpdateRuntime, String>,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut context = tauri::generate_context!();
    let exe = std::env::current_exe().unwrap_or_default();
    let override_dir = if cfg!(debug_assertions) {
        std::env::var_os("PRISM_DEV_DATA_DIR").map(PathBuf::from)
    } else {
        None
    };
    let isolated_test = override_dir.is_some();
    let package_family = if isolated_test {
        Ok(None)
    } else {
        package_context::current_family()
    };
    let portable = override_dir.is_none()
        && matches!(&package_family, Ok(None))
        && app_paths::is_portable(&exe);
    let test_mode = cfg!(debug_assertions)
        && std::env::var_os("PRISM_DEV_TEST_MODE").is_some_and(|value| value == "1");
    // Portable fixtures use real path selection and single-instance handling,
    // but may never access the user's clipboard or open an unattended error dialog.
    if test_mode && !isolated_test && !portable {
        eprintln!("PRISM_DEV_TEST_MODE requires a portable fixture or an isolated data directory.");
        return;
    }
    if portable {
        context.config_mut().identifier =
            app_paths::portable_identifier(&context.config().identifier, &exe);
    } else if let Ok(Some(family)) = &package_family {
        context.config_mut().identifier =
            package_context::instance_identifier(&context.config().identifier, family);
    }
    let builder = tauri::Builder::default();
    let builder = builder.plugin(
        tauri_plugin_updater::Builder::new()
            .pubkey(update_backend::PUBLIC_KEY.trim())
            .build(),
    );
    // Isolated debug tests must not hand their launch to an already running user instance.
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
            let location = package_family.as_ref().map_err(Clone::clone).and_then(|family| {
                let packaged_dir = family.as_ref().map(|family| {
                    let local = app.path().local_data_dir().map_err(|error| error.to_string())?;
                    package_context::data_directory(&local, family)
                }).transpose()?;
                let installed_dir = app.path().app_config_dir().map_err(|error| error.to_string())?;
                app_paths::resolve(&exe, installed_dir, override_dir.clone(), packaged_dir)
            })
                .and_then(|location| {
                    if matches!(location.kind, app_paths::InstallKind::Portable | app_paths::InstallKind::Packaged)
                        && std::env::var_os("WEBVIEW2_USER_DATA_FOLDER").is_some()
                    {
                        Err("This distribution cannot use WEBVIEW2_USER_DATA_FOLDER. Remove that environment override so the WebView profile stays in the application data folder.".into())
                    } else {
                        Ok(location)
                    }
                });
            let location = match location {
                Ok(location) => location,
                Err(error) => {
                    eprintln!("Prism data location failed: {error}");
                    if isolated_test || test_mode {
                        return Err(std::io::Error::other(error).into());
                    }
                    let handle = app.handle().clone();
                    app.dialog().message(error).title("Prism — Data folder unavailable")
                        .kind(MessageDialogKind::Error).show(move |_| handle.exit(1));
                    return Ok(());
                }
            };
            let data_dir = location.dir.clone();
            let (mut settings, config_error) =
                match settings::read_settings(&data_dir.join("settings.json")) {
                    Ok(settings) => (settings, None),
                    Err(error) => (ClientSettings::default(), Some(error)),
                };
            if location.kind == app_paths::InstallKind::Portable {
                settings.start_on_login = false;
            }
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
            let startup_error = if config_error.is_none() {
                commands::apply_autostart(app.handle(), &location, settings.start_on_login).err()
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
            let memory_clipboard = (isolated_test || test_mode)
                .then(clipboard_history::MemoryClipboard::default);
            if let Some(source) = &memory_clipboard {
                clipboard.start(source.boxed(), source.boxed());
            } else {
                clipboard.start(
                    clipboard_history::system_source(app.handle().clone()),
                    clipboard_history::system_source(app.handle().clone()),
                );
            }
            // Keep large local caches out of the roaming settings profile. Tests always use
            // their explicit isolated directory for every service and model asset.
            let asset_dir = if location.kind != app_paths::InstallKind::Installed {
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
                if matches!(location.kind, app_paths::InstallKind::Portable | app_paths::InstallKind::Packaged) {
                    content.exclude_paths(&[data_dir.clone()]);
                }
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
                    asset_dir.join("documents"),
                    asset_dir.join("updates"),
                    asset_dir.join("EBWebView"),
                    asset_dir.join("WebView2"),
                    data_dir.join("webview"),
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
            let documents = rag
                .as_ref()
                .map_err(|_| {
                    document_service::DocumentError::new(
                        document_service::DocumentErrorCode::Storage,
                    )
                })
                .and_then(|rag| document_service::DocumentService::open(&asset_dir, rag.clone()));
            let updates=update_runtime::UpdateRuntime::new(app.handle().clone(),&asset_dir,&location,
                settings.updates.auto_check && config_error.is_none(),isolated_test || test_mode);
            #[cfg(debug_assertions)]
            let input_fixture = (isolated_test || test_mode)
                .then(|| std::sync::Arc::new(paste_fixture::MemoryInput::default()));
            let paste_input: std::sync::Arc<dyn paste_target::InputBackend + Send + Sync> = {
                #[cfg(debug_assertions)]
                if let Some(fixture) = &input_fixture {
                    fixture.clone()
                } else {
                    std::sync::Arc::from(paste_target::system_backend())
                }
                #[cfg(not(debug_assertions))]
                std::sync::Arc::from(paste_target::system_backend())
            };
            app.manage(AppState {
                settings: Mutex::new(settings),
                session: Mutex::new(session),
                shortcut_error: Mutex::new(shortcut_error),
                startup_error: Mutex::new(startup_error),
                config_error: Mutex::new(config_error),
                chat_handoff: Mutex::new(None),
                data_dir,
                location,
                index,
                clipboard,
                paste_targets: std::sync::Arc::new(paste_target::PasteTargets::default()),
                paste_input,
                #[cfg(debug_assertions)]
                input_fixture,
                #[cfg(debug_assertions)]
                clipboard_fixture: memory_clipboard,
                dictionary,
                embedding,
                content,
                rag,
                documents,
                updates,
            });
            if !isolated_test && !test_mode {
                let state = app.state::<AppState>();
                if let Err(error) = foreground_tracker::start(&state.paste_targets) {
                    state.paste_targets.disable();
                    if let Ok(mut message) = state.startup_error.lock() {
                        message.get_or_insert(error);
                    }
                }
            }
            windows::capture_paste_target(app.handle());
            windows::build_main(app.handle())?;
            windows::build_quick(app.handle())?;
            if let Ok(updates)=&app.state::<AppState>().updates {updates.start();}
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
            diagnostics::diagnostics_snapshot,
            diagnostics::diagnostics_open_issue,
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
            clipboard_commands::clipboard_paste,
            clipboard_commands::paste_fixture,
            update_commands::update_status,
            update_commands::update_check,
            update_commands::update_dismiss,
            update_commands::update_install,
            update_commands::update_fixture,
            update_commands::update_open_download,
            update_commands::update_verify_fixture,
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
            content_commands::content_ocr_capabilities,
            content_commands::content_refresh,
            content_commands::content_cancel_index,
            content_commands::content_cancel_search,
            content_commands::search_content,
            content_commands::search_semantic,
            content_commands::search_hybrid,
            content_commands::search_automatic,
            content_commands::content_chunks,
            content_commands::content_file_action,
            content_commands::content_prepare_unlock,
            content_commands::content_unlock_pdf,
            content_commands::content_cancel_unlock,
            rag_commands::rag_create,
            rag_commands::rag_create_from_search,
            rag_commands::rag_create_filename_search,
            rag_commands::rag_create_keyword_search,
            rag_commands::rag_open_main_chat,
            rag_commands::rag_take_chat_handoff,
            rag_commands::rag_file_set_page,
            rag_commands::rag_edit_file_set,
            rag_commands::rag_set_selection,
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
            document_commands::doc_create,
            document_commands::doc_list,
            document_commands::doc_get,
            document_commands::doc_current_citations,
            document_commands::doc_revisions,
            document_commands::doc_rename,
            document_commands::doc_delete,
            document_commands::doc_save_outline,
            document_commands::doc_save_manual,
            document_commands::doc_restore,
            document_commands::doc_export,
            document_commands::doc_generate_outline,
            document_commands::doc_generate_body,
            document_commands::doc_rewrite,
            document_commands::doc_cancel,
            document_commands::doc_job,
            commands::window_action,
            commands::quit_app,
            commands::write_markdown
        ])
        .build(context)
        .expect("Prism could not start")
        .run(|app, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                foreground_tracker::stop();
                if let Some(state) = app.try_state::<AppState>() {
                    if let Ok(updates)=&state.updates {updates.shutdown();}
                    if let Ok(documents) = &state.documents {
                        documents.cancel_all();
                    }
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
