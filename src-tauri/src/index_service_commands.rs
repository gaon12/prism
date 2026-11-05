use crate::{
    index_service::{Action, Error, Issue, Status},
    AppState,
};
use tauri::State;

#[tauri::command]
pub async fn index_service_status(state: State<'_, AppState>) -> Result<Status, Error> {
    let controller = state.index_service.clone();
    let allowed = state.location.kind != crate::app_paths::InstallKind::Isolated;
    tauri::async_runtime::spawn_blocking(move || controller.status(allowed))
        .await
        .map_err(|_| Issue::Unavailable.into())
}
#[tauri::command]
pub async fn index_service_manage(
    state: State<'_, AppState>,
    action: Action,
) -> Result<Status, Error> {
    let controller = state.index_service.clone();
    let allowed = state.location.kind != crate::app_paths::InstallKind::Isolated;
    let result = tauri::async_runtime::spawn_blocking(move || controller.perform(action, allowed))
        .await
        .map_err(|_| Error::from(Issue::Unavailable))?;
    if result.is_ok() {
        let scopes = state
            .settings
            .lock()
            .map_err(|_| Error::from(Issue::Unavailable))?
            .scopes
            .clone();
        state.index.refresh(scopes);
    }
    result
}
