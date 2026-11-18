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
#[cfg(test)]
mod install_regression;
mod journal;
mod maintenance_guard;
mod native_text;
mod operation;
mod payload;
mod previous;
#[cfg(test)]
mod previous_tests;
mod process_control;
mod recovery;
#[cfg(test)]
mod recovery_tests;
mod safe_path;
mod setup_log;
mod system;
mod temp_cleanup;
mod transaction;
mod trust;

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
/// Prism's publisher certificate (`PRISM_TRUSTED_CERT` or `signing/prism-publisher.cer` at
/// build time). Empty when the setup was built without one; then no certificate is touched.
static TRUSTED_CERT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/trusted-publisher.cer"));

/// After a successful install, lets this user's Windows trust Prism's publisher (see
/// [`trust`]) - only with `consented`: the user opted in inside the visible setup window after
/// seeing the certificate's identity. Without it, nothing is added; the state is only logged.
/// Never fails the install.
fn apply_publisher_trust(consented: bool) {
    let result = match system::current().cert_stores() {
        Some(stores) => trust::trust(&*stores, TRUSTED_CERT, consented),
        None => Ok(trust::TrustResult::NotConfigured),
    };
    match result {
        Ok(result) => setup_log::line(format!("publisher certificate: {result:?}")),
        Err(error) => setup_log::line(format!("publisher certificate not trusted: {error}")),
    }
}

/// After a successful uninstall, removes what [`apply_publisher_trust`] added. Never fails it.
fn untrust_publisher(ask: bool) {
    let result = match system::current().cert_stores() {
        Some(stores) => trust::untrust(&*stores, ask),
        None => Ok(()),
    };
    match result {
        Ok(()) => setup_log::line("publisher certificate: removed what setup had added"),
        Err(error) => setup_log::line(format!("publisher certificate not removed: {error}")),
    }
}

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
    recovery_pending: bool,
    previous_version: Option<previous::PreviousVersion>,
    previous_error: Option<String>,
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
    /// This run's log file (shown in the window's log view), if one could be created.
    log_path: Option<PathBuf>,
    /// The same version is already installed here, so "Repair" can rewrite its files.
    repair_available: bool,
    /// The publisher certificate this setup can offer to trust (name, fingerprints, expiry),
    /// to show before the user decides; `null` when there is none. Passing
    /// `trustPublisher: true` to `run_install` is the user's opt-in.
    trust_certificate: Option<trust::CertIdentity>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Progress {
    stage: Stage,
    fraction: f64,
}

/// Progress callback that also writes each new stage to the setup log (once per stage).
fn logged_progress(mut report: impl FnMut(Stage, f64)) -> impl FnMut(Stage, f64) {
    let mut last = None;
    move |stage, fraction| {
        if last != Some(stage) {
            setup_log::line(format!("stage: {stage:?}"));
            last = Some(stage);
        }
        report(stage, fraction);
    }
}

/// Writes how an operation ended to the setup log and passes the result on unchanged.
fn logged<T>(operation: &str, result: Result<T, String>) -> Result<T, String> {
    match &result {
        Ok(_) => setup_log::line(format!("{operation}: succeeded")),
        Err(error) => setup_log::line(format!("{operation}: FAILED: {error}")),
    }
    result
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
    let mode = if setup.args.restore_version.is_some() {
        "restore"
    } else if setup.args.uninstall {
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
    let previous = previous::available(&install_dir);
    let previous_error = previous.as_ref().err().cloned();
    let repair_available = payload_size() > 0
        && installed_version.as_deref() == Some(VERSION)
        && install::read_record(&install_dir).version == VERSION;
    if let Some(expected) = &setup.args.restore_version {
        if !previous
            .as_ref()
            .ok()
            .and_then(|value| value.as_ref())
            .is_some_and(|value| &value.version == expected)
        {
            return Err(
                "The requested previous version is unavailable. No application files were changed."
                    .into(),
            );
        }
    }
    Ok(SetupInfo {
        mode,
        ui: if setup.args.ui == UiMode::Passive && setup.args.restore_version.is_none() {
            "passive"
        } else {
            "full"
        },
        version: VERSION,
        installed_version,
        recovery_pending: recovery::has_workspaces(&install_dir),
        previous_version: previous.unwrap_or_default(),
        previous_error,
        free_bytes: system::free_bytes(&install_dir),
        install_dir,
        default_dir: friendly_path(install::default_install_dir(&folders)),
        desktop_shortcut: options.desktop_shortcut,
        start_menu_shortcut: options.start_menu_shortcut,
        required_bytes,
        payload_ready: payload_size() > 0,
        relaunch: setup.args.relaunch,
        log_path: setup_log::path(),
        repair_available,
        trust_certificate: (!TRUSTED_CERT.is_empty())
            .then(|| trust::identity(TRUSTED_CERT).ok())
            .flatten(),
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FolderCheck {
    error: Option<String>,
    free_bytes: Option<u64>,
    required_bytes: Option<u64>,
    recovery_pending: bool,
    previous_version: Option<previous::PreviousVersion>,
    previous_error: Option<String>,
    /// What the folder holds: empty, installed, damaged, recovery or conflict.
    health: install::FolderHealth,
    /// Version recorded in this folder, if Prism is installed there.
    installed_version: Option<String>,
    /// This setup can repair the folder (same version installed, no recovery or conflict).
    repair_available: bool,
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
            recovery_pending: false,
            previous_version: None,
            previous_error: None,
            health: install::FolderHealth::Conflict,
            installed_version: None,
            repair_available: false,
        };
    }
    let estimate = std::fs::metadata(&setup.exe)
        .map_err(|_| "Could not estimate setup size.".to_string())
        .and_then(|metadata| install::required_free_bytes(&dir, payload_size(), metadata.len()));
    let error = install::validate_install_dir(&dir)
        .err()
        .or_else(|| estimate.as_ref().err().cloned());
    let recovery_pending = error.is_none() && recovery::has_workspaces(&dir);
    let previous = previous::available(&dir);
    let previous_error = previous.as_ref().err().cloned();
    let health = install::folder_health(&dir);
    let installed_version = Some(install::read_record(&dir).version).filter(|v| !v.is_empty());
    let repair_available = payload_size() > 0
        && installed_version.as_deref() == Some(VERSION)
        && matches!(
            health,
            install::FolderHealth::Installed | install::FolderHealth::Damaged
        );
    FolderCheck {
        error,
        health,
        installed_version,
        repair_available,
        free_bytes: system::free_bytes(&dir),
        required_bytes: estimate.ok(),
        recovery_pending,
        previous_version: previous.unwrap_or_default(),
        previous_error,
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
/// `close_running_app`: the user agreed to close the running Prism in this folder.
/// `trust_publisher`: the user ticked "trust this publisher" after seeing its identity.
/// Missing or false: no certificate is added.
async fn run_install(
    app: tauri::AppHandle,
    options: InstallOptions,
    close_running_app: Option<bool>,
    trust_publisher: Option<bool>,
) -> Result<(), String> {
    system::check_sandbox_destination(&options.install_dir).map_err(|error| error.to_string())?;
    if payload_size() == 0 {
        return Err("This setup does not contain Prism. Download it again.".into());
    }
    let operation = app.state::<Setup>().gate.begin()?;
    let exe = app.state::<Setup>().exe.clone();
    let relaunch = app.state::<Setup>().args.relaunch;
    // Only a full window can carry the opt-in; passive runs (updates) never add trust.
    let consented = app.state::<Setup>().args.ui == UiMode::Full && trust_publisher == Some(true);
    setup_log::line(format!(
        "install {VERSION} into {} (desktop shortcut {}, Start menu {})",
        options.install_dir.display(),
        options.desktop_shortcut,
        options.start_menu_shortcut
    ));
    tauri::async_runtime::spawn_blocking(move || {
        let result = logged(
            "install",
            install::install(
                &*system::current_with_app_close(close_running_app.unwrap_or(false)),
                PAYLOAD,
                VERSION,
                &options,
                &exe,
                logged_progress(|stage, fraction| {
                    let _ = app.emit("setup://progress", Progress { stage, fraction });
                }),
            ),
        );
        if let Err(error) = result {
            // The updater closed Prism and asked for a restart (`/R`). The failed update was
            // rolled back, so allow restarting the earlier version when it is intact.
            operation.fail(restart_fallback(relaunch, &options.install_dir));
            return Err(error);
        }
        apply_publisher_trust(consented);
        let installed = safe_path::root(&options.install_dir).map_err(|error| error.to_string())?;
        operation.complete(Some(installed))
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn run_uninstall(
    app: tauri::AppHandle,
    remove_data: bool,
    close_running_app: Option<bool>,
) -> Result<(), String> {
    let operation = app.state::<Setup>().gate.begin()?;
    let Some(dir) = app.state::<Setup>().args.from_temp.clone() else {
        return Err("Start the uninstaller from Apps & features.".into());
    };
    let ask = app.state::<Setup>().args.ui == UiMode::Full;
    system::check_sandbox_destination(&dir).map_err(|error| error.to_string())?;
    setup_log::line(format!(
        "uninstall from {} (remove data: {remove_data})",
        dir.display()
    ));
    tauri::async_runtime::spawn_blocking(move || {
        logged(
            "uninstall",
            install::uninstall(
                &*system::current_with_app_close(close_running_app.unwrap_or(false)),
                &dir,
                remove_data,
                logged_progress(|stage, fraction| {
                    let _ = app.emit("setup://progress", Progress { stage, fraction });
                }),
            ),
        )?;
        untrust_publisher(ask);
        operation.complete(None)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn run_restore(
    app: tauri::AppHandle,
    install_dir: PathBuf,
    expected_version: String,
    close_running_app: Option<bool>,
) -> Result<(), String> {
    system::check_sandbox_destination(&install_dir).map_err(|e| e.to_string())?;
    let operation = app.state::<Setup>().gate.begin()?;
    setup_log::line(format!(
        "restore {expected_version} in {}",
        install_dir.display()
    ));
    tauri::async_runtime::spawn_blocking(move || {
        logged(
            "restore",
            previous::restore(
                &*system::current_with_app_close(close_running_app.unwrap_or(false)),
                &install_dir,
                &expected_version,
                logged_progress(|stage, fraction| {
                    let _ = app.emit("setup://progress", Progress { stage, fraction });
                }),
            ),
        )?;
        let installed = safe_path::root(&install_dir).map_err(|e| e.to_string())?;
        operation.complete(Some(installed))
    })
    .await
    .map_err(|_| "Previous-version restoration failed.".to_string())?
}

/// Repairs the installation in `install_dir`: rewrites every app file from this setup and
/// recreates its shortcuts and "Apps & features" entry. Settings and data are kept.
/// `install::repair` checks, under the folder lock, that the folder records this same version
/// and is the registered installation; otherwise nothing changes.
#[tauri::command]
async fn run_repair(
    app: tauri::AppHandle,
    install_dir: PathBuf,
    close_running_app: Option<bool>,
) -> Result<(), String> {
    if payload_size() == 0 {
        return Err("This setup does not contain Prism. Download it again.".into());
    }
    system::check_sandbox_destination(&install_dir).map_err(|e| e.to_string())?;
    // Keep that folder's own shortcut choices.
    let mut args = app.state::<Setup>().args.clone();
    args.install_dir = Some(install_dir);
    let (options, _) = suggested_options(&args)?;
    let operation = app.state::<Setup>().gate.begin()?;
    let exe = app.state::<Setup>().exe.clone();
    setup_log::line(format!(
        "repair {VERSION} in {}",
        options.install_dir.display()
    ));
    tauri::async_runtime::spawn_blocking(move || {
        let result = logged(
            "repair",
            install::repair(
                &*system::current_with_app_close(close_running_app.unwrap_or(false)),
                PAYLOAD,
                VERSION,
                &options,
                &exe,
                logged_progress(|stage, fraction| {
                    let _ = app.emit("setup://progress", Progress { stage, fraction });
                }),
            ),
        );
        if let Err(error) = result {
            operation.fail(None);
            return Err(error);
        }
        let installed = safe_path::root(&options.install_dir).map_err(|e| e.to_string())?;
        operation.complete(Some(installed))
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Result of checking an installation's files for the repair offer.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ScanResult {
    /// Missing or resized app files (at most `install::MAX_DAMAGED`).
    damaged: Vec<String>,
    /// How many files were actually compared.
    checked: usize,
    /// How many files the installation should have.
    total: usize,
    /// True when the scan stopped at the damaged-file limit before comparing every file.
    truncated: bool,
}

/// Compares the installed files in `install_dir` with this setup's copy (names and sizes).
/// Runs in the background because reading the packed file list takes a few seconds.
#[tauri::command]
async fn scan_installation(install_dir: PathBuf) -> Result<ScanResult, String> {
    system::check_sandbox_destination(&install_dir).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        let expected =
            payload::list(PAYLOAD).map_err(|_| "The setup payload is damaged.".to_string())?;
        let scan = install::damaged_files(&install_dir, VERSION, &expected)?;
        let total = expected.len() + 1;
        setup_log::line(format!(
            "scan {}: {} missing or changed among {} of {} files checked",
            install_dir.display(),
            scan.damaged.len(),
            scan.checked,
            total
        ));
        Ok(ScanResult {
            damaged: scan.damaged,
            checked: scan.checked,
            total,
            truncated: scan.truncated,
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

/// This run's log for the window's log view.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LogView {
    path: Option<PathBuf>,
    /// The last 64 KiB, starting at a whole line when possible.
    text: String,
    /// True when the start of the log was left out.
    truncated: bool,
}

/// Errors (no log file, unreadable file) are returned as messages for the log view.
#[tauri::command]
fn read_log() -> Result<LogView, String> {
    let path = setup_log::path().ok_or("No setup log was created for this run.")?;
    let tail =
        setup_log::tail().map_err(|error| format!("Could not read the setup log: {error}"))?;
    Ok(LogView {
        path: Some(path),
        text: tail.text,
        truncated: tail.truncated,
    })
}

/// Opens the log folder in File Explorer.
#[tauri::command]
fn open_log_folder() -> Result<(), String> {
    let folder = setup_log::path()
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .ok_or("No setup log was created.")?;
    Command::new("explorer")
        .arg(folder)
        .spawn()
        .map(|_| ())
        .map_err(|_| "Could not open the log folder.".into())
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
        let args = &setup.args.relaunch_args;
        if setup.gate.launch_is_fallback() {
            setup_log::line("Restarting the intact earlier Prism version after a failed update.");
        }
        // Recheck and start every launch while holding the folder lock, including a normal
        // successful installation: another setup may have begun since this one finished.
        let started = install::with_intact_installation(&dir, || start_prism(&dir, args));
        if let Err(error) = started {
            setup.gate.cancel_finish();
            return Err(error);
        }
    }
    app.exit(0);
    Ok(())
}

/// After a failed update with `/R`: the folder of the earlier, intact installation to start
/// again, or `None` (no restart requested, or the folder is not clearly intact).
fn restart_fallback(relaunch: bool, dir: &Path) -> Option<PathBuf> {
    if !relaunch || !install::intact_installation(dir) {
        return None;
    }
    safe_path::root(dir).ok()
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
    if let Some(version) = &args.restore_version {
        command.arg("--restore-version").arg(version);
    }
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
    let result = if let Some(version) = &args.restore_version {
        let result = args
            .from_temp
            .clone()
            .map(Ok)
            .unwrap_or_else(|| suggested_options(args).map(|(o, _)| o.install_dir))
            .and_then(|dir| previous::restore(&*system::current(), &dir, version, |_, _| {}));
        if args.from_temp.is_some() {
            match (result, delete_self_later()) {
                (Ok(()), Ok(())) => Ok(()),
                (Err(e), _) | (_, Err(e)) => Err(e),
            }
        } else {
            result
        }
    } else if let Some(dir) = &args.from_temp {
        let result = install::uninstall(&*system::current(), dir, false, |_, _| {});
        if result.is_ok() {
            // Silent: never show Windows' question; a later uninstall can still remove it.
            untrust_publisher(false);
        }
        match (result, delete_self_later()) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), _) | (_, Err(error)) => Err(error),
        }
    } else {
        let exe = env::current_exe().map_err(|error| error.to_string());
        suggested_options(args).and_then(|(options, _)| {
            let result = install::install(
                &*system::current(),
                PAYLOAD,
                VERSION,
                &options,
                &exe?,
                |_, _| {},
            );
            if let Err(error) = result {
                // Same as the window: restart the rolled-back earlier version when asked to.
                // Check and start happen together under the folder lock.
                if let Some(dir) = restart_fallback(args.relaunch, &options.install_dir) {
                    let _ = install::with_intact_installation(&dir, || {
                        start_prism(&dir, &args.relaunch_args)
                    });
                }
                return Err(error);
            }
            // Silent: never adds trust; the state is only logged.
            apply_publisher_trust(false);
            if args.relaunch {
                install::with_intact_installation(&options.install_dir, || {
                    start_prism(&options.install_dir, &args.relaunch_args)
                })?;
            }
            Ok(())
        })
    };
    match logged("silent run", result) {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}

/// Without WebView2 the window cannot open; explain it with a plain Windows message box in the
/// user's display language, naming this run's log file.
#[cfg(windows)]
fn explain_missing_webview() {
    use windows::{
        core::HSTRING,
        Win32::UI::WindowsAndMessaging::{MessageBoxW, IDYES, MB_ICONWARNING, MB_YESNO},
    };
    let language = native_text::pick_language(&native_text::user_locale());
    let log = setup_log::path().map(|path| path.display().to_string());
    let (title, text) = native_text::missing_webview(language, log.as_deref());
    // SAFETY: the strings live until the call returns, and there is no owner window.
    let answer = unsafe {
        MessageBoxW(
            None,
            &HSTRING::from(text),
            &HSTRING::from(title),
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
    // One log file per run. Prism's own /ARGS values are not written, only their count.
    setup_log::start(&*system::current());
    setup_log::line(format!(
        "Prism setup {VERSION} started as {exe_name}: ui {:?}, uninstall {}, update {}",
        args.ui, args.uninstall, args.update
    ));
    setup_log::line(format!(
        "relaunch {} ({} app arguments), folder {:?}, temporary copy for {:?}, restore {:?}",
        args.relaunch,
        args.relaunch_args.len(),
        args.install_dir,
        args.from_temp,
        args.restore_version
    ));

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
        setup_log::line("Microsoft Edge WebView2 is missing; the setup window cannot open.");
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
            run_restore,
            run_repair,
            scan_installation,
            read_log,
            open_log_folder,
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
