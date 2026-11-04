use crate::{
    dictionary_service::{DictionaryError, LookupResponse, PackJob},
    dictionary_store::{InstalledPack, PackManifest},
    AppState,
};
use std::path::PathBuf;
use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;
#[tauri::command]
pub async fn dictionary_open_source(app: AppHandle, url: String) -> Result<(), String> {
    let parsed = tauri::Url::parse(&url).map_err(|_| "Invalid dictionary source URL.")?;
    if url.len() > 2048
        || !["https", "http"].contains(&parsed.scheme())
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err("Invalid dictionary source URL.".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        app.opener()
            .open_url(parsed.as_str(), None::<&str>)
            .map_err(|_| "Could not open the dictionary source.".to_string())
    })
    .await
    .map_err(|_| "Dictionary worker stopped.")?
}
fn service(
    state: &AppState,
) -> Result<crate::dictionary_service::DictionaryService, DictionaryError> {
    state.dictionary.clone()
}
fn worker_error() -> DictionaryError {
    DictionaryError {
        code: "worker".into(),
        message: "Dictionary worker stopped unexpectedly.".into(),
    }
}
#[tauri::command]
pub async fn dictionary_lookup(
    state: State<'_, AppState>,
    language: String,
    word: String,
    offline: bool,
) -> Result<LookupResponse, DictionaryError> {
    let service = service(&state)?;
    tauri::async_runtime::spawn_blocking(move || service.lookup(&language, &word, offline))
        .await
        .map_err(|_| worker_error())?
}
#[tauri::command]
pub async fn dictionary_packs(
    state: State<'_, AppState>,
) -> Result<Vec<InstalledPack>, DictionaryError> {
    let service = service(&state)?;
    tauri::async_runtime::spawn_blocking(move || service.packs())
        .await
        .map_err(|_| worker_error())?
}
#[tauri::command]
pub fn dictionary_job(state: State<'_, AppState>) -> Result<PackJob, DictionaryError> {
    service(&state)?.job()
}
#[tauri::command]
pub fn dictionary_cancel(state: State<'_, AppState>) -> Result<(), DictionaryError> {
    service(&state)?.cancel();
    Ok(())
}
#[tauri::command]
pub async fn dictionary_import(
    state: State<'_, AppState>,
    path: String,
    manifest: Option<PackManifest>,
) -> Result<InstalledPack, DictionaryError> {
    let service = service(&state)?;
    tauri::async_runtime::spawn_blocking(move || {
        service.import_pack(&PathBuf::from(path), manifest)
    })
    .await
    .map_err(|_| worker_error())?
}
#[tauri::command]
pub async fn dictionary_download(
    state: State<'_, AppState>,
    language: String,
) -> Result<InstalledPack, DictionaryError> {
    let service = service(&state)?;
    tauri::async_runtime::spawn_blocking(move || service.download_pack(&language))
        .await
        .map_err(|_| worker_error())?
}
#[tauri::command]
pub async fn dictionary_remove(
    state: State<'_, AppState>,
    id: String,
) -> Result<(), DictionaryError> {
    let service = service(&state)?;
    tauri::async_runtime::spawn_blocking(move || service.remove(&id))
        .await
        .map_err(|_| worker_error())?
}
