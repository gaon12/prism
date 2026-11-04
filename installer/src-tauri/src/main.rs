//! Prism setup: one small program that installs, updates and uninstalls Prism on Windows.
//!
//! How it is started decides what it does (see `args.rs`):
//! - double-click `Prism_x.y.z_setup.exe`  → setup window
//! - Prism's updater (`/P /UPDATE /R ...`)  → progress-only window, then Prism restarts
//! - `/S`                                    → no window at all
//! - `uninstall.exe` (from Apps & features)  → uninstall window (or nothing with `/S`)
//!
//! The window is a small web page in `../ui`, talking to the commands below.

// No console window behind the setup window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod args;
mod folder_lock;
mod install;
mod journal;
#[cfg(test)]
mod install_regression;
mod operation;
mod payload;
mod recovery;
mod safe_path;
mod system;
mod temp_cleanup;
mod transaction;

use args::{SetupArgs, UiMode};
use install::{InstallOptions, Stage, APP_EXE};
use serde::Serialize;
use std::{
    env,
    path::{Path, PathBuf},
    process::{Command, ExitCode},
    sync::Arc,
};
use tauri::{Emitter, Manager};

/// The app files, packed into this program by `build.rs`.
static PAYLOAD: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/payload.bin"));
/// Version of the packed Prism (read from the app's tauri.conf.json at build time).
const VERSION: &str = env!("PRISM_VERSION");
static CLEANUP_WORKER: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/prism-cleanup.exe"));

/// Everything the commands need, created once at start-up.
struct Setup {
    args: SetupArgs,
    /// This program's own path (copied as uninstall.exe when installing).
    exe: PathBuf,
    gate: Arc<operation::Gate>,
}

/// What the window needs to draw its first screen.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SetupInfo {
    /// "install", "update" or "uninstall"
    mode: &'static str,
    /// "full" or "passive"
    ui: &'static str,
    version: &'static str,
    installed_version: Option<String>,
    /// Folder to suggest (the existing installation, or the default one).
    install_dir: PathBuf,
    default_dir: PathBuf,
    desktop_shortcut: bool,
    start_menu_shortcut: bool,
    /// Size of the app files once installed.
    required_bytes: u64,
    free_bytes: Option<u64>,
    /// False in development builds made without the app; installing is then refused.
    payload_ready: bool,
    relaunch: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Progress {
    stage: Stage,
    fraction: f64,
}

fn friendly_path(path: PathBuf) -> PathBuf {
    let value = path.to_string_lossy();
    if let Some(rest) = value.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = value.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        path
    }
}

/// Install folder and shortcut choices to suggest, based on any existing installation.
fn suggested_options(args: &SetupArgs) -> Result<(InstallOptions, Option<String>), String> {
    let folders = system::current()
        .folders()
        .map_err(|error| error.to_string())?;
    let existing = system::current().existing_install();
    let installed_version = existing.as_ref().and_then(|(_, version)| version.clone());
    let install_dir = args
        .install_dir
        .clone()
        .or_else(|| existing.map(|(dir, _)| dir))
        .unwrap_or_else(|| install::default_install_dir(&folders));
    system::check_sandbox_destination(&install_dir).map_err(|error| error.to_string())?;
    let record = install::read_record(&install_dir);
    // Keep the user's earlier shortcut choices; a first install creates both shortcuts.
    let (desktop, start_menu) = if record.version.is_empty() {
        (true, true)
    } else {
        let has = |link: PathBuf| record.shortcuts.contains(&link);
        (
            has(folders.desktop.join("Prism.lnk")),
            has(folders.start_menu.join("Prism.lnk")),
        )
    };
    Ok((
        InstallOptions {
            install_dir,
            desktop_shortcut: desktop,
            start_menu_shortcut: start_menu,
        },
        installed_version,
    ))
}

fn payload_size() -> u64 {
    // Build-time metadata avoids decompressing the entire app on the UI/IPC thread.
    env!("PRISM_PAYLOAD_SIZE").parse().unwrap_or(0)
}

#[tauri::command]
fn setup_info(setup: tauri::State<'_, Setup>) -> Result<SetupInfo, String> {
    let (options, installed_version) = suggested_options(&setup.args)?;
    let folders = system::current()
        .folders()
        .map_err(|error| error.to_string())?;
    let mode = if setup.args.uninstall {
        "uninstall"
    } else if setup.args.update || installed_version.is_some() {
        "update"
    } else {
        "install"
    };
    let install_dir = friendly_path(match &setup.args.from_temp {
        Some(dir) => dir.clone(),
        None => options.install_dir,
    });
    let required_bytes = install::required_free_bytes(
        &install_dir,
        payload_size(),
        std::fs::metadata(&setup.exe)
            .map_err(|_| "Could not estimate setup size.")?
            .len(),
    )?;
    Ok(SetupInfo {
        mode,
        ui: if setup.args.ui == UiMode::Passive {
            "passive"
        } else {
            "full"
        },
        version: VERSION,
        installed_version,
        free_bytes: system::free_bytes(&install_dir),
        install_dir,
        default_dir: friendly_path(install::default_install_dir(&folders)),
        desktop_shortcut: options.desktop_shortcut,
        start_menu_shortcut: options.start_menu_shortcut,
        required_bytes,
        payload_ready: payload_size() > 0,
        relaunch: setup.args.relaunch,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FolderCheck {
    error: Option<String>,
    free_bytes: Option<u64>,
    required_bytes: Option<u64>,
}

/// Checks a folder the user typed or picked, so the window can warn before installing.
#[tauri::command]
fn check_folder(path: String, setup: tauri::State<'_, Setup>) -> FolderCheck {
    let dir = PathBuf::from(path.trim());
    if let Err(error) = system::check_sandbox_destination(&dir) {
        return FolderCheck {
            error: Some(error.to_string()),
            free_bytes: None,
            required_bytes: None,
        };
    }
    let estimate = std::fs::metadata(&setup.exe)
        .map_err(|_| "Could not estimate setup size.".to_string())
        .and_then(|metadata| install::required_free_bytes(&dir, payload_size(), metadata.len()));
    let error = install::validate_install_dir(&dir)
        .err()
        .or_else(|| estimate.as_ref().err().cloned());
    FolderCheck {
        error,
        free_bytes: system::free_bytes(&dir),
        required_bytes: estimate.ok(),
    }
}

/// Opens the Windows folder picker. When the user picks a non-empty folder, a "Prism"
/// subfolder is suggested instead, like most installers do.
#[tauri::command]
async fn choose_folder(app: tauri::AppHandle, current: String) -> Option<PathBuf> {
    use tauri_plugin_dialog::DialogExt;
    let start = Path::new(&current)
        .ancestors()
        .find(|folder| folder.exists())
        .map(Path::to_path_buf);
    let mut dialog = app.dialog().file();
    if let Some(start) = start {
        dialog = dialog.set_directory(start);
    }
    let picked = dialog.blocking_pick_folder()?.into_path().ok()?;
    let is_prism_folder = picked.join(APP_EXE).exists();
    let is_empty = std::fs::read_dir(&picked).is_ok_and(|mut entries| entries.next().is_none());
    let named_prism = picked
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("Prism"));
    Some(if is_prism_folder || is_empty || named_prism {
        picked
    } else {
        picked.join("Prism")
    })
}

/// Installs in a background thread and reports progress as `setup://progress` events.
#[tauri::command]
async fn run_install(app: tauri::AppHandle, options: InstallOptions) -> Result<(), String> {
    system::check_sandbox_destination(&options.install_dir).map_err(|error| error.to_string())?;
    if payload_size() == 0 {
        return Err("This setup does not contain Prism. Download it again.".into());
    }
    let operation = app.state::<Setup>().gate.begin()?;
    let exe = app.state::<Setup>().exe.clone();
    tauri::async_runtime::spawn_blocking(move || {
        install::install(
            &*system::current(),
            PAYLOAD,
            VERSION,
            &options,
            &exe,
            |stage, fraction| {
                let _ = app.emit("setup://progress", Progress { stage, fraction });
            },
        )?;
        let installed = safe_path::root(&options.install_dir).map_err(|error| error.to_string())?;
        operation.complete(Some(installed))
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn run_uninstall(app: tauri::AppHandle, remove_data: bool) -> Result<(), String> {
    let operation = app.state::<Setup>().gate.begin()?;
    let Some(dir) = app.state::<Setup>().args.from_temp.clone() else {
        return Err("Start the uninstaller from Apps & features.".into());
    };
    system::check_sandbox_destination(&dir).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        install::uninstall(&*system::current(), &dir, remove_data, |stage, fraction| {
            let _ = app.emit("setup://progress", Progress { stage, fraction });
        })?;
        operation.complete(None)
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Closes the setup, optionally starting the installed Prism first.
#[tauri::command]
fn finish(
    app: tauri::AppHandle,
    setup: tauri::State<'_, Setup>,
    launch: bool,
) -> Result<(), String> {
    let target = setup.gate.finish(launch)?;
    if let Some(dir) = target {
        if let Err(error) = start_prism(&dir, &setup.args.relaunch_args) {
            setup.gate.cancel_finish();
            return Err(error);
        }
    }
    app.exit(0);
    Ok(())
}

fn start_prism(dir: &Path, args: &[String]) -> Result<(), String> {
    let target = safe_path::file(dir, APP_EXE)
        .map_err(|_| "The installed application path is unsafe.".to_string())?;
    Command::new(target)
        .args(args)
        .current_dir(dir)
        .spawn()
        .map(|_| ())
        .map_err(|_| {
            "Prism could not be started. You can launch it from its installation folder.".into()
        })
}

/// A running program cannot delete its own file on Windows. The uninstaller therefore copies
/// itself to %TEMP%, starts that copy (which can delete the install folder) and exits.
fn restart_from_temp(args: &SetupArgs) -> ExitCode {
    let Ok(exe) = env::current_exe() else {
        return ExitCode::FAILURE;
    };
    let Some(dir) = exe.parent() else {
        return ExitCode::FAILURE;
    };
    let copy = match temp_cleanup::copy_uninstaller(&exe) {
        Ok(copy) => copy,
        Err(_) => return ExitCode::FAILURE,
    };
    let mut command = Command::new(&copy);
    command.arg("--from-temp").arg(dir);
    if args.ui == UiMode::Silent {
        command.arg("/S");
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    match command.spawn() {
        Ok(_) => ExitCode::SUCCESS,
        Err(_) => {
            if let Ok(directory) = temp_cleanup::own_directory(&copy) {
                let _ = temp_cleanup::remove_released_copy(&directory);
            }
            ExitCode::FAILURE
        }
    }
}

/// A native worker retries removal after this owned temporary copy exits.
fn delete_self_later() -> Result<(), String> {
    let exe = env::current_exe()
        .map_err(|_| "Could not locate the temporary uninstaller.".to_string())?;
    let directory = temp_cleanup::own_directory(&exe)
        .map_err(|_| "Temporary uninstall cleanup refused an unowned path.".to_string())?;
    let helper = temp_cleanup::deploy_worker(CLEANUP_WORKER)
        .map_err(|_| "Could not prepare temporary uninstall cleanup.".to_string())?;
    let mut command = Command::new(helper);
    command.arg("--cleanup-temp").arg(directory);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    command
        .spawn()
        .map(|_| ())
        .map_err(|_| "Could not start temporary uninstall cleanup.".into())
}

/// `/S`: does the whole job without any window. Exit code 0 means success.
fn run_silent(args: &SetupArgs) -> ExitCode {
    let result = if let Some(dir) = &args.from_temp {
        let result = install::uninstall(&*system::current(), dir, false, |_, _| {});
        match (result, delete_self_later()) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), _) | (_, Err(error)) => Err(error),
        }
    } else {
        let exe = env::current_exe().map_err(|error| error.to_string());
        suggested_options(args).and_then(|(options, _)| {
            install::install(
                &*system::current(),
                PAYLOAD,
                VERSION,
                &options,
                &exe?,
                |_, _| {},
            )?;
            if args.relaunch {
                start_prism(&options.install_dir, &args.relaunch_args)?;
            }
            Ok(())
        })
    };
    if result.is_ok() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Without WebView2 the window cannot open; explain it with a plain Windows message box.
#[cfg(windows)]
fn explain_missing_webview() {
    use windows::{
        core::w,
        Win32::UI::WindowsAndMessaging::{MessageBoxW, IDYES, MB_ICONWARNING, MB_YESNO},
    };
    // SAFETY: static strings and no owner window.
    let answer = unsafe {
        MessageBoxW(
            None,
            w!("Prism needs Microsoft Edge WebView2, which is missing on this PC.\n\nOpen the download page now?"),
            w!("Prism setup"),
            MB_YESNO | MB_ICONWARNING,
        )
    };
    if answer == IDYES {
        let _ = Command::new("explorer")
            .arg("https://developer.microsoft.com/microsoft-edge/webview2/")
            .spawn();
    }
}

fn main() -> ExitCode {
    let all: Vec<String> = env::args().skip(1).collect();
    let exe = env::current_exe().unwrap_or_default();
    let exe_name = exe
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let args = SetupArgs::parse(&all, &exe_name);
    // An empty or invalid sandbox must never fall back to real Windows integration.
    if system::sandbox_root().is_err() {
        return ExitCode::FAILURE;
    }

    if let Some(dir) = &args.from_temp {
        if system::check_sandbox_destination(dir).is_err() {
            return ExitCode::FAILURE;
        }
    }
    if args.from_temp.is_some() && temp_cleanup::own_directory(&exe).is_err() {
        return ExitCode::FAILURE;
    }
    if args.uninstall && args.from_temp.is_none() {
        return restart_from_temp(&args);
    }
    if args.ui == UiMode::Silent {
        return run_silent(&args);
    }
    if !system::webview2_installed() {
        #[cfg(windows)]
        explain_missing_webview();
        return ExitCode::FAILURE;
    }

    let from_temp = args.from_temp.is_some();
    let gate = Arc::new(operation::Gate::default());
    let window_gate = gate.clone();
    let exit_gate = gate.clone();
    let result = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(Setup { args, exe, gate })
        .setup(|app| {
            let config = app
                .config()
                .app
                .windows
                .first()
                .ok_or("Setup window configuration is missing.")?;
            let builder = tauri::WebviewWindowBuilder::from_config(app, config)?;
            #[cfg(windows)]
            let builder = if let Some(root) = system::sandbox_root()? {
                let profile = root.join("SetupWebView");
                safe_path::no_links(&profile)?;
                builder.data_directory(profile)
            } else {
                builder
            };
            builder.build()?;
            Ok(())
        })
        .on_window_event(move |_, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window_gate.busy() {
                    api.prevent_close();
                } else {
                    let _ = window_gate.finish(false);
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            setup_info,
            check_folder,
            choose_folder,
            run_install,
            run_uninstall,
            finish
        ])
        .build(tauri::generate_context!());
    let Ok(app) = result else {
        return ExitCode::FAILURE;
    };
    app.run(move |_, event| {
        match event {
            tauri::RunEvent::ExitRequested { api, .. } => {
                if exit_gate.busy() {
                    api.prevent_exit();
                } else {
                    let _ = exit_gate.finish(false);
                }
            }
            // App::run exits the process directly; code after it cannot schedule cleanup.
            tauri::RunEvent::Exit if from_temp => {
                let _ = delete_self_later();
            }
            _ => {}
        }
    });
    ExitCode::SUCCESS
}
