use crate::{
    collection_service::{CollectionService, ImportProgress, Media},
    collection_store::*,
    AppState,
};
use std::path::PathBuf;
use tauri::State;
use tauri_plugin_opener::OpenerExt;
#[tauri::command]
pub async fn collection_get(state: State<'_, AppState>, id: String) -> Result<Collection> {
    let store = service(&state)?.store;
    blocking(move || store.collection(&id)).await
}
#[tauri::command]
pub async fn collection_work_get(state: State<'_, AppState>, id: String) -> Result<Work> {
    let store = service(&state)?.store;
    blocking(move || store.work(&id)).await
}
#[tauri::command]
pub async fn collection_unit_get(state: State<'_, AppState>, id: String) -> Result<Unit> {
    let store = service(&state)?.store;
    blocking(move || store.unit(&id)).await
}
#[tauri::command]
pub async fn collection_page_address(
    state: State<'_, AppState>,
    work_id: String,
    page_id: String,
) -> Result<PageAddress> {
    let store = service(&state)?.store;
    blocking(move || store.page_address(&work_id, &page_id)).await
}

fn service(state: &State<'_, AppState>) -> Result<CollectionService> {
    state.collections.clone()
}
async fn blocking<T: Send + 'static>(
    operation: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tauri::async_runtime::spawn_blocking(operation)
        .await
        .map_err(|_| error(ErrorCode::Storage))?
}
#[tauri::command]
pub async fn collection_office_status(
    state: State<'_, AppState>,
) -> Result<crate::collection_office::Status> {
    let service = service(&state)?;
    blocking(move || service.office_status()).await
}
#[tauri::command]
pub async fn collection_office_install(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    path: String,
) -> Result<()> {
    use tauri::Emitter;
    let service = service(&state)?;
    let snapshot = service.clone();
    let notifications = app.clone();
    let result = blocking(move || {
        service.install_office(&PathBuf::from(path), &|progress| {
            let _ = app.emit("prism:office-runtime", progress);
            if let Ok(status) = service.office_status() {
                let _ = app.emit("prism:office-status", status);
            }
        })
    })
    .await;
    // Always publish after the active guard drops, including errors/cancel and
    // a worker panic. Other windows need a terminal notification as well.
    if let Ok(status) = snapshot.office_status() {
        let _ = notifications.emit("prism:office-status", status);
    }
    result
}
#[tauri::command]
pub async fn collection_office_cancel(state: State<'_, AppState>) -> Result<()> {
    service(&state)?.cancel_office_install();
    Ok(())
}
#[tauri::command]
pub async fn collection_list(
    state: State<'_, AppState>,
    offset: u64,
    limit: u32,
) -> Result<Listing<Collection>> {
    let store = service(&state)?.store;
    blocking(move || store.collections(offset, limit)).await
}
#[tauri::command]
pub async fn collection_create(
    state: State<'_, AppState>,
    title: String,
    description: String,
) -> Result<Collection> {
    let store = service(&state)?.store;
    blocking(move || store.create_collection(&title, &description)).await
}
#[tauri::command]
pub async fn collection_update(
    state: State<'_, AppState>,
    id: String,
    expected: u64,
    title: String,
    description: String,
) -> Result<Collection> {
    let store = service(&state)?.store;
    blocking(move || store.update_collection(&id, expected, &title, &description)).await
}
#[tauri::command]
pub async fn collection_works(
    state: State<'_, AppState>,
    collection_id: String,
    offset: u64,
    limit: u32,
) -> Result<Listing<Work>> {
    let store = service(&state)?.store;
    blocking(move || store.works(&collection_id, offset, limit)).await
}
#[tauri::command]
pub async fn collection_work_create(
    state: State<'_, AppState>,
    collection_id: String,
    title: String,
    metadata: WorkMetadata,
) -> Result<Work> {
    let store = service(&state)?.store;
    blocking(move || store.create_work(&collection_id, &title, metadata)).await
}
#[tauri::command]
pub async fn collection_work_update(
    state: State<'_, AppState>,
    id: String,
    expected: u64,
    title: String,
    metadata: WorkMetadata,
) -> Result<Work> {
    let store = service(&state)?.store;
    blocking(move || store.update_work(&id, expected, &title, metadata)).await
}
#[tauri::command]
pub async fn collection_units(
    state: State<'_, AppState>,
    work_id: String,
    offset: u64,
    limit: u32,
) -> Result<Listing<Unit>> {
    let store = service(&state)?.store;
    blocking(move || store.units(&work_id, offset, limit)).await
}
#[tauri::command]
pub async fn collection_unit_create(
    state: State<'_, AppState>,
    work_id: String,
    title: String,
    kind: UnitKind,
) -> Result<Unit> {
    let store = service(&state)?.store;
    blocking(move || store.create_unit(&work_id, &title, kind)).await
}
#[tauri::command]
pub async fn collection_unit_update(
    state: State<'_, AppState>,
    id: String,
    expected: u64,
    title: String,
    kind: UnitKind,
) -> Result<Unit> {
    let store = service(&state)?.store;
    blocking(move || store.update_unit(&id, expected, &title, kind)).await
}
#[tauri::command]
pub async fn collection_sources(
    state: State<'_, AppState>,
    unit_id: String,
) -> Result<Vec<Source>> {
    let store = service(&state)?.store;
    blocking(move || store.sources(&unit_id)).await
}
#[tauri::command]
pub async fn collection_import(
    state: State<'_, AppState>,
    unit_id: String,
    expected: u64,
    paths: Vec<String>,
) -> Result<Unit> {
    let service = service(&state)?;
    blocking(move || {
        service.import(
            &unit_id,
            expected,
            paths.into_iter().map(PathBuf::from).collect(),
        )
    })
    .await
}
#[tauri::command]
pub fn collection_cancel(state: State<'_, AppState>, unit_id: String) -> Result<()> {
    service(&state)?.cancel(&unit_id);
    Ok(())
}
#[tauri::command]
pub fn collection_jobs(state: State<'_, AppState>) -> Result<Vec<ImportProgress>> {
    Ok(service(&state)?.jobs())
}
#[tauri::command]
pub async fn collection_pages(
    state: State<'_, AppState>,
    unit_id: String,
    expected: u64,
    offset: u64,
    limit: u32,
) -> Result<Listing<Page>> {
    let store = service(&state)?.store;
    blocking(move || store.pages(&unit_id, expected, offset, limit)).await
}
#[tauri::command]
pub async fn collection_media(
    state: State<'_, AppState>,
    page_id: String,
    thumbnail: bool,
) -> Result<Media> {
    let service = service(&state)?;
    blocking(move || service.media(&page_id, thumbnail)).await
}
#[tauri::command]
pub async fn collection_navigation(
    state: State<'_, AppState>,
    page_id: String,
    expected: u64,
) -> Result<crate::collection_service::Navigation> {
    let service = service(&state)?;
    blocking(move || service.navigation(&page_id, expected)).await
}
#[tauri::command]
pub async fn collection_cover(state: State<'_, AppState>, work_id: String) -> Result<Media> {
    let service = service(&state)?;
    blocking(move || service.cover(&work_id)).await
}
#[tauri::command]
pub async fn collection_cache_epub(
    state: State<'_, AppState>,
    page_id: String,
    source_hash: String,
    data_url: String,
) -> Result<Media> {
    let service = service(&state)?;
    blocking(move || service.cache_epub(&page_id, &source_hash, &data_url)).await
}
#[tauri::command]
pub async fn collection_cache_text(
    state: State<'_, AppState>,
    page_id: String,
    source_hash: String,
    data_url: String,
) -> Result<Media> {
    let service = service(&state)?;
    blocking(move || service.cache_text(&page_id, &source_hash, &data_url)).await
}
#[tauri::command]
pub async fn collection_reading(
    state: State<'_, AppState>,
    work_id: String,
) -> Result<ReadingPosition> {
    let store = service(&state)?.store;
    blocking(move || store.reading(&work_id)).await
}
#[tauri::command]
pub async fn collection_save_reading(
    state: State<'_, AppState>,
    work_id: String,
    page_id: Option<String>,
    completed: bool,
) -> Result<ReadingPosition> {
    let store = service(&state)?.store;
    blocking(move || store.save_reading(&work_id, page_id.as_deref(), completed)).await
}
#[tauri::command]
pub async fn collection_remove(
    state: State<'_, AppState>,
    kind: String,
    id: String,
    expected: u64,
) -> Result<()> {
    let service = service(&state)?;
    blocking(move || service.remove(&kind, &id, expected)).await
}
#[tauri::command]
pub async fn collection_move_work(
    state: State<'_, AppState>,
    id: String,
    expected: u64,
    collection_id: String,
) -> Result<Work> {
    let store = service(&state)?.store;
    blocking(move || store.move_work(&id, expected, &collection_id)).await
}
#[tauri::command]
pub async fn collection_move_unit(
    state: State<'_, AppState>,
    id: String,
    expected: u64,
    before: Option<String>,
) -> Result<Unit> {
    let store = service(&state)?.store;
    blocking(move || store.move_unit_before(&id, expected, before.as_deref())).await
}
#[tauri::command]
pub async fn collection_move_page(
    state: State<'_, AppState>,
    unit_id: String,
    expected: u64,
    page_id: String,
    before: Option<String>,
) -> Result<Unit> {
    let store = service(&state)?.store;
    blocking(move || store.move_page_before(&unit_id, expected, &page_id, before.as_deref())).await
}
#[tauri::command]
pub async fn collection_relink(
    state: State<'_, AppState>,
    id: String,
    expected: u64,
    path: String,
) -> Result<Source> {
    let service = service(&state)?;
    blocking(move || service.relink(&id, expected, &PathBuf::from(path))).await
}
#[tauri::command]
pub async fn collection_remove_source(
    state: State<'_, AppState>,
    id: String,
    expected: u64,
) -> Result<()> {
    let service = service(&state)?;
    blocking(move || {
        let source = service.store.source(&id)?;
        service.cancel(&source.unit_id);
        service.store.remove_source(&id, expected)
    })
    .await
}
#[tauri::command]
pub async fn collection_open_source(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    id: String,
    reveal: bool,
) -> Result<()> {
    let service = service(&state)?;
    let path = blocking(move || service.source_path(&id)).await?;
    if reveal {
        app.opener()
            .reveal_item_in_dir(&path)
            .map_err(|_| error(ErrorCode::SourceChanged))
    } else {
        app.opener()
            .open_path(
                crate::path_identity::normal(&path).to_string_lossy(),
                None::<&str>,
            )
            .map_err(|_| error(ErrorCode::SourceChanged))
    }
}
