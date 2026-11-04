use crate::{
    update_runtime::{UpdateRuntime, UpdateView},
    AppState,
};
use tauri::State;
use tauri_plugin_opener::OpenerExt;
fn service(state: &AppState) -> Result<UpdateRuntime, String> {
    state.updates.as_ref().cloned().map_err(Clone::clone)
}
#[tauri::command]
pub fn update_status(state: State<'_, AppState>) -> Result<UpdateView, String> {
    Ok(service(&state)?.status())
}
#[tauri::command]
pub async fn update_check(state: State<'_, AppState>) -> Result<UpdateView, String> {
    let service = service(&state)?;
    tauri::async_runtime::spawn_blocking(move || service.check())
        .await
        .map_err(|_| "Update check failed.".into())
}
#[tauri::command]
pub fn update_dismiss(state: State<'_, AppState>) -> Result<UpdateView, String> {
    Ok(service(&state)?.dismiss())
}
#[tauri::command]
pub async fn update_install(state: State<'_, AppState>) -> Result<(), String> {
    let service = service(&state)?;
    tauri::async_runtime::spawn_blocking(move || service.install())
        .await
        .map_err(|_| "Update request failed.")?
}
#[tauri::command]
pub fn update_open_download(app: tauri::AppHandle) -> Result<(), String> {
    app.opener()
        .open_url(crate::update_backend::DOWNLOAD_PAGE, None::<&str>)
        .map_err(|_| "Could not open the download page.".into())
}
#[tauri::command]
pub fn update_verify_fixture(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    public_key: String,
    manifest: serde_json::Value,
    payload: Vec<u8>,
) -> Result<(), String> {
    #[cfg(debug_assertions)]
    {
        service(&state)?.fixture("snapshot")?;
        if public_key.len() > 8192
            || payload.len() > 65536
            || serde_json::to_vec(&manifest)
                .map_err(|_| "Invalid fixture.")?
                .len()
                > 65536
        {
            return Err("Update fixture is too large.".into());
        }
        use tauri_plugin_updater::UpdaterExt;
        let updater = app
            .updater_builder()
            .pubkey(public_key)
            .build()
            .map_err(|_| "Invalid verification fixture configuration.")?;
        let update = updater
            .restore_update(manifest)
            .map_err(|_| "Invalid verification fixture manifest.")?
            .ok_or("No compatible verification fixture.")?;
        return update
            .verify(&payload)
            .map_err(|_| "Update signature or signed version verification failed.".into());
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = (state, app, public_key, manifest, payload);
        Err("Update fixtures require an isolated debug profile.".into())
    }
}
#[tauri::command]
pub fn update_fixture(
    state: State<'_, AppState>,
    mode: String,
) -> Result<serde_json::Value, String> {
    #[cfg(debug_assertions)]
    {
        return service(&state)?.fixture(&mode);
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = (state, mode);
        Err("Update fixtures require an isolated debug profile.".into())
    }
}
