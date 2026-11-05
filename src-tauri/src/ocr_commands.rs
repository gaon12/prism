use crate::{ocr, ocr_runtime::Status, AppState};
use std::path::PathBuf;
use tauri::State;

#[tauri::command]
pub fn ocr_status(state: State<'_, AppState>) -> ocr::Result<Status> {
    Ok(state.ocr_runtime.as_ref().map_err(Clone::clone)?.status())
}
#[tauri::command]
pub async fn ocr_install(
    state: State<'_, AppState>,
    offline_directory: Option<String>,
) -> ocr::Result<Status> {
    let runtime = state.ocr_runtime.clone()?;
    let offline = offline_directory.map(PathBuf::from);
    if offline.as_ref().is_some_and(|path| !path.is_absolute()) {
        return Err(ocr::error(ocr::Code::InvalidInput));
    }
    // Model installation never modifies OCR preferences, starts inference or
    // resumes a paused index. A later normal scan observes the model cache tag.
    tauri::async_runtime::spawn_blocking(move || runtime.install(offline.as_deref()))
        .await
        .map_err(|_| ocr::error(ocr::Code::Failed))?
}
#[tauri::command]
pub fn ocr_cancel_install(state: State<'_, AppState>) -> ocr::Result<()> {
    state
        .ocr_runtime
        .as_ref()
        .map_err(Clone::clone)?
        .cancel_install();
    Ok(())
}
