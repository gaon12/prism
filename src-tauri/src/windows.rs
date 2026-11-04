use crate::AppState;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

pub fn build_quick(app: &AppHandle) -> tauri::Result<()> {
    let font_size = app
        .state::<AppState>()
        .settings
        .lock()
        .map(|settings| settings.font_size)
        .unwrap_or(14);
    let extra = f64::from(font_size.saturating_sub(14));
    WebviewWindowBuilder::new(app, "quick", WebviewUrl::App("index.html#quick".into()))
        .title("Prism — 빠른 검색")
        .inner_size(720.0 + extra * 20.0, 480.0 + extra * 24.0)
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

pub fn show_main_window(app: &AppHandle) -> Result<(), String> {
    let main = app
        .get_webview_window("main")
        .ok_or("Main window is unavailable.")?;
    main.show().map_err(|error| error.to_string())?;
    main.unminimize().map_err(|error| error.to_string())?;
    main.set_focus().map_err(|error| error.to_string())?;
    if let Some(quick) = app.get_webview_window("quick") {
        let _ = quick.hide();
    }
    Ok(())
}

pub fn show_quick_window(app: &AppHandle) -> Result<(), String> {
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
    quick.center().map_err(|error| error.to_string())?;
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
