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
    builder.build()?;
    Ok(())
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
