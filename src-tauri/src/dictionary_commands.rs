use crate::{
    dictionary_service::{DictionaryError, LookupResponse, PackJob},
    dictionary_store::{InstalledPack, PackManifest},
    AppState,
};
use std::path::PathBuf;
use tauri::State;
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
