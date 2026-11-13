use crate::AppState;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

pub fn build_main(app: &AppHandle) -> tauri::Result<()> {
    let config = app
        .config()
        .app
        .windows
        .iter()
        .find(|window| window.label == "main")
        .ok_or(tauri::Error::WindowNotFound)?;
    let mut builder = WebviewWindowBuilder::from_config(app, config)?;
    if let Some(dir) = app.state::<AppState>().location.webview_dir() {
        builder = builder.data_directory(dir);
    }
    #[cfg(windows)]
    if let Some(args) = browser_args() {
        builder = builder.additional_browser_args(&args);
    }
    builder.build()?;
    Ok(())
}

/// Machines with this much physical memory or less draw the WebView in
/// software (see `software_rendering`).
const LOW_MEMORY_BYTES: u64 = 4 << 30;

/// Whether this run draws its WebView windows without the GPU.
///
/// The WebView2 GPU process alone holds over 100 MB while Prism shows
/// results. On a small machine that is a fifth of everything Prism may use
/// (measured: 1 CPU / 512 MB, whole C: drive, peak 470-478 MB with the GPU,
/// 372 MB without, at the same scan time and CPU cost). Drawing in software
/// costs some animation smoothness, which is the better trade there, so it is
/// only chosen when the machine has 4 GiB of memory or less.
/// `PRISM_WEBVIEW_SOFTWARE=1` or `=0` forces either choice (for testing).
/// Decided once, because every window must share the same arguments.
pub fn software_rendering() -> bool {
    static CHOICE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *CHOICE.get_or_init(|| {
        let forced = std::env::var("PRISM_WEBVIEW_SOFTWARE").ok();
        // Only WebView2 takes the argument; elsewhere nothing would change.
        cfg!(windows) && choose_software_rendering(forced.as_deref(), total_memory())
    })
}

/// The decision itself: an explicit `1`/`0` wins, otherwise low memory.
/// Unknown memory keeps the default GPU drawing.
fn choose_software_rendering(forced: Option<&str>, total_memory: Option<u64>) -> bool {
    match forced {
        Some("1") => true,
        Some("0") => false,
        _ => total_memory.is_some_and(|bytes| bytes <= LOW_MEMORY_BYTES),
    }
}

/// Installed physical memory in bytes.
#[cfg(windows)]
fn total_memory() -> Option<u64> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut status = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..unsafe { std::mem::zeroed() }
    };
    // SAFETY: `status` is a correctly sized, writable MEMORYSTATUSEX.
    (unsafe { GlobalMemoryStatusEx(&mut status) } != 0).then_some(status.ullTotalPhys)
}
/// Only WebView2 takes browser arguments; other platforms keep their engine.
#[cfg(not(windows))]
fn total_memory() -> Option<u64> {
    None
}

/// WebView2 arguments for every Prism window.
///
/// Passing arguments replaces wry's default ones, so those are repeated:
/// they turn off Edge's mini menu, PDF mini menu and SmartScreen in the app.
/// Prism sets no proxy or autoplay, the only other things wry would add.
///
/// The two backgrounding switches keep the windows' renderer at normal
/// priority. Chromium otherwise drops a hidden or occluded renderer to the
/// idle priority class (thread priority 4), below Prism's own below-normal
/// indexing threads (7). The hidden quick-search window shares its renderer
/// with the main window, so on one CPU the main window stayed blank until the
/// first whole-drive collection finished: ready at 12-17 s, against 2.3-3.2 s
/// in four of four runs with these switches. Hidden windows still trim memory
/// through `set_memory_target`.
#[cfg(windows)]
fn browser_args() -> Option<String> {
    Some(browser_args_for(software_rendering()))
}

/// The argument string; `--disable-gpu` only when drawing in software.
#[cfg(windows)]
fn browser_args_for(software: bool) -> String {
    let mut args = [
        "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection",
        "--disable-renderer-backgrounding",
        "--disable-backgrounding-occluded-windows",
    ]
    .join(" ");
    if software {
        args.push_str(" --disable-gpu");
    }
    args
}

/// Tell a window's WebView2 how hard to hold on to memory.
///
/// WebView2 keeps caches and freed JavaScript memory around for speed. When a
/// window is inactive (hidden in the tray, the quick search waiting for its
/// hotkey, or just behind another app) the "low" target asks it to trim them,
/// which matters on small (512 MB) machines. Activating the window restores
/// the normal target. Microsoft documents exactly this inactive/active use.
/// Other platforms have no equivalent switch, so this does nothing there.
pub fn set_memory_target(app: &AppHandle, label: &str, active: bool) {
    #[cfg(windows)]
    if let Some(window) = app.get_webview_window(label) {
        use webview2_com::Microsoft::Web::WebView2::Win32::{
            ICoreWebView2_19, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW,
            COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL,
        };
        use windows::core::Interface;
        let level = if active {
            COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL
        } else {
            COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW
        };
        // Runs on the UI thread. An older WebView2 runtime without the
        // interface (before 1.0.1774) simply keeps its default behavior.
        let _ = window.with_webview(move |webview| unsafe {
            if let Ok(core) = webview
                .controller()
                .CoreWebView2()
                .and_then(|core| core.cast::<ICoreWebView2_19>())
            {
                let _ = core.SetMemoryUsageTargetLevel(level);
            }
        });
    }
    #[cfg(not(windows))]
    let _ = (app, label, active);
}

pub fn build_quick(app: &AppHandle) -> tauri::Result<()> {
    let font_size = app
        .state::<AppState>()
        .settings
        .lock()
        .map(|settings| settings.font_size)
        .unwrap_or(14);
    let extra = f64::from(font_size.saturating_sub(14));
    let expanded = app
        .state::<AppState>()
        .session
        .lock()
        .map(|session| !session.query.trim().is_empty())
        .unwrap_or(false);
    let mut builder =
        WebviewWindowBuilder::new(app, "quick", WebviewUrl::App("index.html#quick".into()));
    if let Some(dir) = app.state::<AppState>().location.webview_dir() {
        builder = builder.data_directory(dir);
    }
    // Both windows share one WebView2 environment: same arguments.
    #[cfg(windows)]
    if let Some(args) = browser_args() {
        builder = builder.additional_browser_args(&args);
    }
    builder
        .title("Prism — 빠른 검색")
        .inner_size(
            720.0 + extra * 20.0,
            if expanded {
                480.0 + extra * 24.0
            } else {
                94.0 + extra * 4.0
            },
        )
        .decorations(false)
        .transparent(true)
        .shadow(true)
        .resizable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible(false)
        .focused(false)
        .center()
        .build()?;
    Ok(())
}

pub fn resize_quick(app: &AppHandle, expanded: bool) -> Result<(), String> {
    let font_size = app
        .state::<AppState>()
        .settings
        .lock()
        .map_err(|_| "Settings are unavailable.")?
        .font_size;
    let extra = f64::from(font_size.saturating_sub(14));
    if let Some(quick) = app.get_webview_window("quick") {
        let width = 720.0 + extra * 20.0;
        let height = if expanded {
            480.0 + extra * 24.0
        } else {
            94.0 + extra * 4.0
        };
        let (width, height) = if let Some(monitor) =
            quick.current_monitor().map_err(|error| error.to_string())?
        {
            let area = monitor.work_area();
            (
                width.min((area.size.width as f64 / monitor.scale_factor() - 24.0).max(320.0)),
                height.min((area.size.height as f64 / monitor.scale_factor() - 24.0).max(180.0)),
            )
        } else {
            (width, height)
        };
        quick
            .set_size(tauri::LogicalSize::new(width, height))
            .map_err(|error| error.to_string())?;
        position_quick(&quick)?;
    }
    Ok(())
}

fn position_quick(quick: &tauri::WebviewWindow) -> Result<(), String> {
    if let Some(monitor) = quick.current_monitor().map_err(|error| error.to_string())? {
        let area = monitor.work_area();
        let size = quick.outer_size().map_err(|error| error.to_string())?;
        // Keep the input at the same anchor when results expand, while fitting the work area.
        let x = area.position.x + (area.size.width as i32 - size.width as i32) / 2;
        let available = (area.size.height as i32 - size.height as i32).max(0);
        let y = area.position.y + ((area.size.height as f64 * 0.18) as i32).min(available);
        quick
            .set_position(tauri::PhysicalPosition::new(x, y))
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

pub fn capture_paste_target(app: &AppHandle) {
    if let Some(state) = app.try_state::<AppState>() {
        state.paste_targets.capture(state.paste_input.as_ref());
    }
}

pub fn show_main_window(app: &AppHandle) -> Result<(), String> {
    let main = app
        .get_webview_window("main")
        .ok_or("Main window is unavailable.")?;
    capture_paste_target(app);
    main.show().map_err(|error| error.to_string())?;
    main.unminimize().map_err(|error| error.to_string())?;
    main.set_focus().map_err(|error| error.to_string())?;
    if let Some(quick) = app.get_webview_window("quick") {
        let _ = quick.hide();
    }
    Ok(())
}

pub fn show_quick_window(app: &AppHandle) -> Result<(), String> {
    if app
        .state::<AppState>()
        .updates
        .as_ref()
        .is_ok_and(|updates| updates.is_installing())
    {
        return Err("An application update is in progress.".into());
    }
    capture_paste_target(app);
    let state = app.state::<AppState>();
    let remember = state
        .settings
        .lock()
        .map_err(|_| "Settings are unavailable.")?
        .remember_search;
    if !remember {
        let mut session = state
            .session
            .lock()
            .map_err(|_| "Search state is unavailable.")?;
        session.query.clear();
        session.selected_path = None;
        app.emit("prism:session", &*session)
            .map_err(|error| error.to_string())?;
    }
    let quick = app
        .get_webview_window("quick")
        .ok_or("Quick window is unavailable.")?;
    let expanded = !state
        .session
        .lock()
        .map_err(|_| "Search state is unavailable.")?
        .query
        .trim()
        .is_empty();
    resize_quick(app, expanded)?;
    position_quick(&quick)?;
    quick.show().map_err(|error| error.to_string())?;
    quick.set_focus().map_err(|error| error.to_string())?;
    quick
        .emit("prism:focus-search", ())
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub fn toggle_quick(app: &AppHandle) -> Result<(), String> {
    if let Some(quick) = app.get_webview_window("quick") {
        if quick.is_visible().map_err(|error| error.to_string())? {
            return quick.hide().map_err(|error| error.to_string());
        }
    }
    show_quick_window(app)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn software_rendering_is_for_small_machines_unless_forced() {
        let gib = 1u64 << 30;
        assert!(choose_software_rendering(None, Some(2 * gib)));
        assert!(choose_software_rendering(None, Some(4 * gib)));
        assert!(!choose_software_rendering(None, Some(4 * gib + 1)));
        assert!(!choose_software_rendering(None, Some(16 * gib)));
        // Unknown memory keeps the GPU.
        assert!(!choose_software_rendering(None, None));
        // An explicit choice wins either way; anything else is ignored.
        assert!(choose_software_rendering(Some("1"), Some(16 * gib)));
        assert!(!choose_software_rendering(Some("0"), Some(2 * gib)));
        assert!(choose_software_rendering(Some("yes"), Some(2 * gib)));
        assert!(!choose_software_rendering(Some("yes"), Some(16 * gib)));
    }

    #[cfg(windows)]
    #[test]
    fn every_window_keeps_its_renderer_at_normal_priority() {
        for software in [false, true] {
            let args = browser_args_for(software);
            let switches: Vec<&str> = args.split(' ').collect();
            // wry's own defaults are kept, plus the two backgrounding switches.
            for expected in [
                "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection",
                "--disable-renderer-backgrounding",
                "--disable-backgrounding-occluded-windows",
            ] {
                assert!(switches.contains(&expected), "{expected} in {args}");
            }
            assert_eq!(switches.contains(&"--disable-gpu"), software, "{args}");
            assert!(switches.iter().all(|switch| !switch.is_empty()), "{args}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn this_machine_reports_its_memory() {
        assert!(total_memory().is_some_and(|bytes| bytes >= 256 << 20));
    }
}
