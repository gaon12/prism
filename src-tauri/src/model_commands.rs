use crate::{embedding_runtime::EmbeddingStatus, AppState};
use std::path::PathBuf;
use tauri::State;

#[tauri::command]
pub async fn model_status(state: State<'_, AppState>) -> Result<EmbeddingStatus, String> {
    let model = state.embedding.clone()?;
    tauri::async_runtime::spawn_blocking(move || model.status())
        .await
        .map_err(|_| "Embedding worker stopped.".to_string())
}
#[tauri::command]
pub fn model_cancel(state: State<'_, AppState>) -> Result<(), String> {
    state
        .embedding
        .as_ref()
        .map_err(Clone::clone)?
        .cancel_download();
    Ok(())
}
#[tauri::command]
pub async fn model_download(state: State<'_, AppState>) -> Result<EmbeddingStatus, String> {
    let model = state.embedding.clone()?;
    tauri::async_runtime::spawn_blocking(move || model.download_assets())
        .await
        .map_err(|_| "Embedding worker stopped.".to_string())?
}
#[tauri::command]
pub async fn model_import(
    state: State<'_, AppState>,
    path: String,
    runtime: bool,
) -> Result<EmbeddingStatus, String> {
    let model = state.embedding.clone()?;
    tauri::async_runtime::spawn_blocking(move || {
        if runtime {
            model.import_runtime(&PathBuf::from(path))
        } else {
            model.import_model(&PathBuf::from(path))
        }
    })
    .await
    .map_err(|_| "Embedding worker stopped.".to_string())?
}
#[tauri::command]
pub async fn model_start(state: State<'_, AppState>) -> Result<EmbeddingStatus, String> {
    let model = state.embedding.clone()?;
    let content = state.content.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let status = model.start()?;
        if let Ok(content) = content {
            let _ = content.refresh();
        }
        Ok(status)
    })
    .await
    .map_err(|_| "Embedding worker stopped.".to_string())?
}
#[tauri::command]
pub async fn model_stop(state: State<'_, AppState>) -> Result<EmbeddingStatus, String> {
    let model = state.embedding.clone()?;
    tauri::async_runtime::spawn_blocking(move || model.stop())
        .await
        .map_err(|_| "Embedding worker stopped.".to_string())?
}
