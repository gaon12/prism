//! Local library orchestration. Registration is an explicit permission to view
//! those references, never a content-index/AI/external-transmission permission.
use crate::{collection_import as import, collection_render, collection_store::*};
use base64::Engine;
use serde::Serialize;
use std::{
    collections::HashMap,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::UNIX_EPOCH,
};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportProgress {
    pub unit_id: String,
    pub phase: String,
    pub processed: u64,
    pub error: Option<ErrorCode>,
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, CollectionService, Collection, Work, Unit) {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("app");
        fs::create_dir(&app).unwrap();
        let service = CollectionService::open(&app, |_| {}).unwrap();
        let collection = service.store.create_collection("자료", "").unwrap();
        let work = service
            .store
            .create_work(&collection.id, "작품", WorkMetadata::default())
            .unwrap();
        let unit = service
            .store
            .create_unit(&work.id, "제1화", UnitKind::Chapter)
            .unwrap();
        (dir, service, collection, work, unit)
    }
    fn png(path: &Path, color: [u8; 4]) {
        image::RgbaImage::from_pixel(80, 120, image::Rgba(color))
            .save(path)
            .unwrap();
    }
    fn decoded(media: &Media) -> image::RgbaImage {
        let data = media
            .data_url
            .as_ref()
            .unwrap()
            .strip_prefix("data:image/webp;base64,")
            .unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data)
            .unwrap();
        image::load_from_memory(&bytes).unwrap().to_rgba8()
    }
    #[test]
    fn real_folder_first_page_cache_hash_and_reference_removal() {
        let (dir, service, collection, work, unit) = fixture();
        let folder = dir.path().join("원본");
        fs::create_dir(&folder).unwrap();
        png(&folder.join("10.png"), [0, 0, 255, 255]);
        png(&folder.join("2.png"), [255, 0, 0, 255]);
        let unit = service
            .import(&unit.id, unit.revision, vec![folder.clone()])
            .unwrap();
        assert_eq!(unit.page_count, 2);
        let pages = service
            .store
            .pages(&unit.id, unit.revision, 0, 10)
            .unwrap()
            .items;
        assert_eq!(pages[0].location.entry.as_deref(), Some("2.png"));
        assert_eq!(
            decoded(&service.cover(&work.id).unwrap()).get_pixel(0, 0).0,
            [255, 0, 0, 255]
        );
        assert_eq!(
            decoded(&service.cover(&work.id).unwrap()).dimensions(),
            (80, 120)
        );
        png(&folder.join("2.png"), [0, 255, 0, 255]);
        assert_eq!(
            service.cover(&work.id).unwrap_err().code,
            ErrorCode::SourceChanged,
            "cached bytes cannot bypass source authentication"
        );
        service.import(&unit.id, unit.revision, vec![]).unwrap();
        assert_eq!(
            decoded(&service.cover(&work.id).unwrap()).get_pixel(0, 0).0,
            [0, 255, 0, 255]
        );
        service
            .remove("collection", &collection.id, collection.revision)
            .unwrap();
        assert!(folder.join("2.png").is_file() && folder.join("10.png").is_file());
        assert!(fs::read_dir(service.inner.cache.as_ref().unwrap())
            .unwrap()
            .flatten()
            .all(|e| owned_cache(&e.file_name().to_string_lossy()).is_none()));
    }
    #[test]
    fn archive_pages_are_naturally_ordered_and_never_extracted() {
        use zip::write::SimpleFileOptions;
        let (dir, service, _, work, unit) = fixture();
        let image = dir.path().join("color.png");
        png(&image, [70, 80, 90, 255]);
        let data = fs::read(&image).unwrap();
        let zip_path = dir.path().join("book.cbz");
        let mut zip = zip::ZipWriter::new(fs::File::create(&zip_path).unwrap());
        for name in ["10.png", "2.png"] {
            zip.start_file(name, SimpleFileOptions::default()).unwrap();
            zip.write_all(&data).unwrap();
        }
        zip.finish().unwrap();
        let unit = service
            .import(&unit.id, unit.revision, vec![zip_path.clone()])
            .unwrap();
        let pages = service
            .store
            .pages(&unit.id, unit.revision, 0, 10)
            .unwrap()
            .items;
        assert_eq!(pages[0].location.entry.as_deref(), Some("2.png"));
        assert_eq!(
            decoded(&service.cover(&work.id).unwrap()).get_pixel(0, 0).0,
            [70, 80, 90, 255]
        );
        assert!(!dir.path().join("2.png").exists());
        let mut zip = zip::ZipWriter::new(fs::File::create(&zip_path).unwrap());
        zip.start_file("../escape.png", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(&data).unwrap();
        zip.finish().unwrap();
        assert_eq!(
            service
                .import(&unit.id, unit.revision, vec![])
                .unwrap_err()
                .code,
            ErrorCode::Input
        );
        assert!(!dir.path().join("escape.png").exists());
        assert_eq!(
            service.store.unit(&unit.id).unwrap().page_count,
            2,
            "a rejected new manifest preserves the old one"
        );
    }
    #[test]
    fn imports_do_not_adopt_internal_cache_and_shutdown_cancels_new_work() {
        let (dir, service, _, _, unit) = fixture();
        let internal = dir.path().join("app/private.png");
        png(&internal, [0, 0, 0, 255]);
        assert_eq!(
            service
                .import(&unit.id, unit.revision, vec![internal])
                .unwrap_err()
                .code,
            ErrorCode::UnsafePath
        );
        let updated = service.store.unit(&unit.id).unwrap();
        assert!(service.store.sources(&unit.id).unwrap().is_empty());
        service.cancel_all();
        assert_eq!(
            service
                .import(&unit.id, updated.revision, vec![])
                .unwrap_err()
                .code,
            ErrorCode::Cancelled
        );
    }
    #[test]
    fn epub_spine_and_cached_viewports_remain_bound_to_current_references() {
        let (dir, service, _, work, unit) = fixture();
        let path = dir.path().join("소설.epub");
        let original = include_bytes!("../../tests/fixtures/epub/spine.epub");
        fs::write(&path, original).unwrap();
        let unit = service
            .import(&unit.id, unit.revision, vec![path.clone()])
            .unwrap();
        assert_eq!(unit.page_count, 2);
        let pages = service
            .store
            .pages(&unit.id, unit.revision, 0, 10)
            .unwrap()
            .items;
        assert_eq!(
            pages[0].location.entry.as_deref(),
            Some("OPS/chapters/2-first.xhtml")
        );
        let media = service.cover(&work.id).unwrap();
        assert_eq!(media.page_id, pages[0].id);
        assert!(media.data_url.is_none() && media.document.is_some());
        assert!(media.document.unwrap().html.contains("첫 번째 장"));
        // Only a cache-admission fixture; actual browser/first-page pixels are
        // verified by the bundled native EPUB UI test, not this generated color.
        let image = dir.path().join("viewport.png");
        png(&image, [210, 40, 50, 255]);
        let bytes =
            collection_render::webp(&fs::read(image).unwrap(), 480, &AtomicBool::new(false))
                .unwrap();
        let data = format!(
            "data:image/webp;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        );
        let saved = service
            .cache_epub(&pages[0].id, &pages[0].source_hash, &data)
            .unwrap();
        assert!(saved.data_url.is_some());
        assert!(service.cover(&work.id).unwrap().document.is_none());
        assert!(service
            .media(&pages[0].id, false)
            .unwrap()
            .document
            .is_some());
        service
            .store
            .save_reading(&work.id, Some(&pages[1].id), false)
            .unwrap();
        assert_eq!(
            service.store.reading(&work.id).unwrap().page_id.as_deref(),
            Some(pages[1].id.as_str())
        );
        fs::write(&path, b"changed source").unwrap();
        assert_eq!(
            service.cover(&work.id).unwrap_err().code,
            ErrorCode::SourceChanged
        );
        assert_eq!(
            service
                .cache_epub(&pages[0].id, &pages[0].source_hash, &data)
                .unwrap_err()
                .code,
            ErrorCode::SourceChanged
        );
        assert!(service.import(&unit.id, unit.revision, vec![]).is_err());
        assert_eq!(service.store.unit(&unit.id).unwrap().page_count, 2);
        assert_eq!(
            service.store.reading(&work.id).unwrap().page_id.as_deref(),
            Some(pages[1].id.as_str())
        );
        assert_eq!(fs::read(&path).unwrap(), b"changed source");
    }
    #[test]
    fn text_viewport_admission_preserves_reader_and_authenticates_cached_originals() {
        let (dir, service, collection, work, unit) = fixture();
        let path = dir.path().join("소설.txt");
        let original = "첫 장의 한국어 원문\n<script>literal</script>\n".repeat(180);
        fs::write(&path, &original).unwrap();
        let unit = service
            .import(&unit.id, unit.revision, vec![path.clone()])
            .unwrap();
        let pages = service
            .store
            .pages(&unit.id, unit.revision, 0, 10)
            .unwrap()
            .items;
        assert!(pages.len() > 1);
        let first = service.cover(&work.id).unwrap();
        assert_eq!(first.page_id, pages[0].id);
        assert!(first.text.as_deref().unwrap().starts_with("첫 장"));
        assert!(first.data_url.is_none());
        // Cache-admission fixture only: real text pixels are independently
        // checked by the bundled collection-text UI scenario.
        let png_path = dir.path().join("viewport.png");
        png(&png_path, [120, 130, 140, 255]);
        let bytes =
            collection_render::webp(&fs::read(png_path).unwrap(), 480, &AtomicBool::new(false))
                .unwrap();
        let data = format!(
            "data:image/webp;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        );
        assert_eq!(
            service
                .cache_epub(&pages[0].id, &pages[0].source_hash, &data)
                .unwrap_err()
                .code,
            ErrorCode::SourceChanged
        );
        assert_eq!(
            service
                .cache_text(&pages[0].id, &"0".repeat(64), &data)
                .unwrap_err()
                .code,
            ErrorCode::SourceChanged
        );
        assert_eq!(
            service
                .cache_text(&pages[0].id, &pages[0].source_hash, "data:text/plain,x")
                .unwrap_err()
                .code,
            ErrorCode::Input
        );
        service
            .cache_text(&pages[0].id, &pages[0].source_hash, &data)
            .unwrap();
        assert_eq!(
            decoded(&service.cover(&work.id).unwrap()).get_pixel(0, 0).0,
            [120, 130, 140, 255]
        );
        assert_eq!(service.media(&pages[0].id, false).unwrap().text, first.text);
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
        fs::write(&path, "changed original").unwrap();
        assert_eq!(
            service.cover(&work.id).unwrap_err().code,
            ErrorCode::SourceChanged
        );
        assert_eq!(
            service
                .cache_text(&pages[0].id, &pages[0].source_hash, &data)
                .unwrap_err()
                .code,
            ErrorCode::SourceChanged
        );
        fs::write(&path, &original).unwrap();
        service
            .remove("collection", &collection.id, collection.revision)
            .unwrap();
        assert!(service
            .cache_text(&pages[0].id, &pages[0].source_hash, &data)
            .is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
    }
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Media {
    pub page_id: String,
    pub source_hash: String,
    pub data_url: Option<String>,
    pub text: Option<String>,
    pub document: Option<crate::collection_epub::Document>,
}
struct Active {
    cancel: Arc<AtomicBool>,
    work: String,
    collection: String,
}
struct Inner {
    jobs: Mutex<HashMap<String, Active>>,
    // Large decoders are bounded independently of import jobs. This lock also
    // serializes generated-cache writes/eviction across both app windows.
    render: Mutex<()>,
    cache: Option<PathBuf>,
    excluded: PathBuf,
    shutdown: AtomicBool,
    emit: Box<dyn Fn(ImportProgress) + Send + Sync>,
}
#[derive(Clone)]
pub struct CollectionService {
    pub store: CollectionStore,
    inner: Arc<Inner>,
}
struct Job {
    inner: Arc<Inner>,
    unit: String,
    cancel: Arc<AtomicBool>,
}
impl Drop for Job {
    fn drop(&mut self) {
        let mut jobs = self.inner.jobs.lock().unwrap_or_else(|e| e.into_inner());
        if jobs
            .get(&self.unit)
            .is_some_and(|job| Arc::ptr_eq(&job.cancel, &self.cancel))
        {
            jobs.remove(&self.unit);
        }
    }
}
const CACHE_MARKER: &[u8] =
    b"Prism collection page cache v1\n<page-sha256>-<source-sha256>-<480|1600>.webp\n<cache-name>.<hex32>.part\n";
const CACHE_BUDGET: u64 = 256 << 20;
fn cache_open(directory: &Path) -> Option<PathBuf> {
    fs::create_dir_all(directory).ok()?;
    let directory = crate::path_identity::canonical(directory).ok()?;
    let marker = directory.join("format");
    match fs::symlink_metadata(&marker) {
        Ok(_) if marked(&directory) => (),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if fs::read_dir(&directory).ok()?.next().is_some() {
                return None;
            }
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(marker)
                .ok()?;
            file.write_all(CACHE_MARKER).ok()?;
            file.sync_all().ok()?;
        }
        _ => return None,
    }
    for entry in fs::read_dir(&directory).ok()?.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str().and_then(|s| s.strip_suffix(".part")) else {
            continue;
        };
        let Some((prefix, nonce)) = name.rsplit_once('.') else {
            continue;
        };
        if owned_cache(prefix).is_some()
            && nonce.len() == 32
            && nonce.bytes().all(|b| b.is_ascii_hexdigit())
            && crate::path_identity::no_links(&entry.path()).is_ok()
        {
            let _ = fs::remove_file(entry.path());
        }
    }
    Some(directory)
}
fn marked(directory: &Path) -> bool {
    let marker = directory.join("format");
    if crate::path_identity::no_links(&marker).is_err() {
        return false;
    }
    let Ok(file) = fs::File::open(marker) else {
        return false;
    };
    let mut bytes = Vec::new();
    file.take(CACHE_MARKER.len() as u64 + 1)
        .read_to_end(&mut bytes)
        .is_ok()
        && bytes == CACHE_MARKER
}
fn cache_name(page: &Page, edge: u32) -> String {
    format!("{}-{}-{edge}.webp", page.id, page.source_hash)
}
fn owned_cache(name: &str) -> Option<(&str, &str)> {
    let stem = name.strip_suffix(".webp")?;
    let mut parts = stem.split('-');
    let page = parts.next()?;
    let source = parts.next()?;
    let edge = parts.next()?;
    if parts.next().is_some()
        || page.len() != 64
        || source.len() != 64
        || !page
            .bytes()
            .chain(source.bytes())
            .all(|b| b.is_ascii_hexdigit())
        || !["480", "1600"].contains(&edge)
    {
        return None;
    }
    Some((page, source))
}
impl CollectionService {
    pub fn open(
        directory: &Path,
        emit: impl Fn(ImportProgress) + Send + Sync + 'static,
    ) -> Result<Self> {
        let root = directory.join("collections");
        let store = CollectionStore::open(&root)?;
        let excluded =
            crate::path_identity::canonical(directory).map_err(|_| error(ErrorCode::Storage))?;
        let cache = cache_open(&root.join("pages"));
        Ok(Self {
            store,
            inner: Arc::new(Inner {
                jobs: Mutex::new(HashMap::new()),
                render: Mutex::new(()),
                cache,
                excluded,
                shutdown: AtomicBool::new(false),
                emit: Box::new(emit),
            }),
        })
    }
    fn notify(&self, unit: &str, phase: &str, processed: u64, issue: Option<ErrorCode>) {
        (self.inner.emit)(ImportProgress {
            unit_id: unit.into(),
            phase: phase.into(),
            processed,
            error: issue,
        });
    }
    fn begin(&self, unit: &str, expected: u64) -> Result<Job> {
        import::check(&self.inner.shutdown)?;
        let unit = self.store.unit(unit)?;
        if unit.revision != expected {
            return Err(error(ErrorCode::Conflict));
        }
        let work = self.store.work(&unit.work_id)?;
        let mut jobs = self
            .inner
            .jobs
            .lock()
            .map_err(|_| error(ErrorCode::Storage))?;
        if jobs.len() >= 2 || jobs.contains_key(&unit.id) {
            return Err(error(ErrorCode::Busy));
        }
        let cancel = Arc::new(AtomicBool::new(false));
        jobs.insert(
            unit.id.clone(),
            Active {
                cancel: cancel.clone(),
                work: work.id,
                collection: work.collection_id,
            },
        );
        Ok(Job {
            inner: self.inner.clone(),
            unit: unit.id,
            cancel,
        })
    }
    /// Existing manifests remain intact until the entire replacement is ready.
    /// A cancelled import retains its references so it can be explicitly retried.
    pub fn import(&self, unit_id: &str, expected: u64, paths: Vec<PathBuf>) -> Result<Unit> {
        let job = self.begin(unit_id, expected)?;
        self.notify(unit_id, "reading", 0, None);
        let mut revision = expected;
        let result = (|| {
            if paths.len() > 10_000
                || paths.iter().map(|p| p.as_os_str().len()).sum::<usize>() > 8 << 20
            {
                return Err(error(ErrorCode::Limit));
            }
            if !paths.is_empty() {
                let mut references = Vec::new();
                for path in paths {
                    import::check(&job.cancel)?;
                    let path = import::canonical(&path, &self.inner.excluded)?;
                    let kind = import::source_kind(&path)?;
                    references.push((crate::path_identity::normal(&path), kind));
                }
                references.sort_by(|a, b| {
                    import::natural(&a.0.to_string_lossy(), &b.0.to_string_lossy())
                });
                self.store.add_sources(unit_id, revision, &references)?;
                revision = self.store.unit(unit_id)?.revision;
            }
            let sources = self.store.sources(unit_id)?;
            let mut pages = Vec::new();
            for source in sources {
                let base = pages.len() as u64;
                let mut last = std::time::Instant::now();
                let next = import::manifest(
                    &source,
                    &self.inner.excluded,
                    &job.cancel,
                    &mut |processed| {
                        if last.elapsed().as_millis() >= 150 {
                            self.notify(unit_id, "reading", base + processed, None);
                            last = std::time::Instant::now();
                        }
                    },
                )?;
                if pages.len().saturating_add(next.len()) > 250_000 {
                    return Err(error(ErrorCode::Limit));
                }
                pages.extend(next);
            }
            import::check(&job.cancel)?;
            self.store
                .replace_pages(unit_id, revision, pages, &job.cancel)
        })();
        if let Err(issue) = &result {
            let _ = self
                .store
                .failed_import(unit_id, revision, issue.code == ErrorCode::Cancelled);
        }
        // A status snapshot taken after this terminal event must never still
        // contain this job. Remove it before publishing terminal progress.
        drop(job);
        match &result {
            Ok(unit) => self.notify(unit_id, "ready", unit.page_count, None),
            Err(issue) => self.notify(unit_id, "failed", 0, Some(issue.code.clone())),
        }
        result
    }
    pub fn cancel(&self, unit: &str) {
        if let Ok(jobs) = self.inner.jobs.lock() {
            if let Some(job) = jobs.get(unit) {
                job.cancel.store(true, Ordering::Release);
            }
        }
    }
    pub fn cancel_all(&self) {
        self.inner.shutdown.store(true, Ordering::Release);
        if let Ok(jobs) = self.inner.jobs.lock() {
            for job in jobs.values() {
                job.cancel.store(true, Ordering::Release);
            }
        }
    }
    pub fn jobs(&self) -> Vec<ImportProgress> {
        self.inner
            .jobs
            .lock()
            .map(|jobs| {
                jobs.keys()
                    .map(|unit| ImportProgress {
                        unit_id: unit.clone(),
                        phase: "reading".into(),
                        processed: 0,
                        error: None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
    pub fn relink(&self, id: &str, expected: u64, path: &Path) -> Result<Source> {
        let path = import::canonical(path, &self.inner.excluded)?;
        let kind = import::source_kind(&path)?;
        let source = self.store.source(id)?;
        self.cancel(&source.unit_id);
        self.store
            .relink_source(id, expected, &crate::path_identity::normal(&path), kind)
    }
    pub fn source_path(&self, id: &str) -> Result<PathBuf> {
        let source = self.store.source(id)?;
        import::canonical(Path::new(&source.path), &self.inner.excluded)
    }
    pub fn remove(&self, kind: &str, id: &str, expected: u64) -> Result<()> {
        if let Ok(jobs) = self.inner.jobs.lock() {
            for (unit, job) in jobs.iter() {
                if (kind == "unit" && unit == id)
                    || (kind == "work" && job.work == id)
                    || (kind == "collection" && job.collection == id)
                {
                    job.cancel.store(true, Ordering::Release);
                }
            }
        }
        match kind {
            "collection" => self.store.remove_collection(id, expected),
            "work" => self.store.remove_work(id, expected),
            "unit" => self.store.remove_unit(id, expected),
            _ => Err(error(ErrorCode::Input)),
        }?;
        self.prune_cache();
        Ok(())
    }
    fn cache_read(&self, page: &Page, edge: u32) -> Option<Vec<u8>> {
        let directory = self.inner.cache.as_ref()?;
        if !marked(directory) {
            return None;
        }
        crate::path_identity::no_links(directory).ok()?;
        let path = directory.join(cache_name(page, edge));
        crate::path_identity::no_links(&path).ok()?;
        let file = fs::File::open(path).ok()?;
        if !file.metadata().ok()?.is_file() {
            return None;
        }
        let mut bytes = Vec::new();
        file.take((16 << 20) + 1).read_to_end(&mut bytes).ok()?;
        (bytes.len() <= 16 << 20 && bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP"))
            .then_some(bytes)
    }
    fn cache_write(&self, page: &Page, edge: u32, bytes: &[u8]) {
        let Some(directory) = &self.inner.cache else {
            return;
        };
        if !marked(directory) {
            return;
        }
        if crate::path_identity::no_links(directory).is_err() {
            return;
        }
        let stage = directory.join(format!(
            "{}.{}.part",
            cache_name(page, edge),
            match id() {
                Ok(id) => id,
                Err(_) => return,
            }
        ));
        let result = (|| -> std::io::Result<()> {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&stage)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            drop(file);
            self.evict(bytes.len() as u64);
            fs::rename(&stage, directory.join(cache_name(page, edge)))
        })();
        if result.is_err() {
            let _ = fs::remove_file(stage);
        }
    }
    fn evict(&self, incoming: u64) {
        let Some(directory) = &self.inner.cache else {
            return;
        };
        if !marked(directory) {
            return;
        }
        let Ok(entries) = fs::read_dir(directory) else {
            return;
        };
        let mut files = Vec::new();
        let mut total = incoming;
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if owned_cache(name).is_none() {
                continue;
            }
            if crate::path_identity::no_links(&entry.path()).is_err() {
                continue;
            }
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            total = total.saturating_add(meta.len());
            files.push((
                meta.modified().unwrap_or(UNIX_EPOCH),
                meta.len(),
                entry.path(),
            ));
        }
        files.sort_unstable_by_key(|x| x.0);
        for (_, length, path) in files {
            if total <= CACHE_BUDGET {
                break;
            }
            if fs::remove_file(path).is_ok() {
                total = total.saturating_sub(length);
            }
        }
    }
    fn prune_cache(&self) {
        let Ok(_guard) = self.inner.render.try_lock() else {
            return;
        };
        let Some(directory) = &self.inner.cache else {
            return;
        };
        if !marked(directory) {
            return;
        }
        if crate::path_identity::no_links(directory).is_err() {
            return;
        }
        let Ok(entries) = fs::read_dir(directory) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some((page, source)) = name.to_str().and_then(owned_cache) else {
                continue;
            };
            if crate::path_identity::no_links(&entry.path()).is_err() {
                continue;
            }
            let obsolete = match self.store.page(page) {
                Ok(page) => page.source_hash != source,
                Err(e) => e.code == ErrorCode::NotFound,
            };
            if obsolete {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
    pub fn media(&self, page_id: &str, thumbnail: bool) -> Result<Media> {
        let _guard = self
            .inner
            .render
            .lock()
            .map_err(|_| error(ErrorCode::Storage))?;
        let page = self.store.page(page_id)?;
        let source = self.store.source(&page.source_id)?;
        let cancel = &self.inner.shutdown;
        import::check(cancel)?;
        if page.location.kind == PageKind::Epub {
            let bytes = import::image_bytes(&source, &page, &self.inner.excluded, cancel)?;
            let cached = thumbnail.then(|| self.cache_read(&page, 480)).flatten();
            let document = if cached.is_none() {
                Some(
                    crate::collection_epub::Book::open(&bytes, cancel)?.document(
                        page.location.number,
                        page.location
                            .entry
                            .as_deref()
                            .ok_or_else(|| error(ErrorCode::Input))?,
                    )?,
                )
            } else {
                None
            };
            self.check_current(&page, &source)?;
            return Ok(Media {
                page_id: page.id,
                source_hash: page.source_hash,
                data_url: cached.map(|bytes| {
                    format!(
                        "data:image/webp;base64,{}",
                        base64::engine::general_purpose::STANDARD.encode(bytes)
                    )
                }),
                text: None,
                document,
            });
        }
        if matches!(page.location.kind, PageKind::Text) {
            if thumbnail {
                if let Some(bytes) = self.cache_read(&page, 480) {
                    // A cached text viewport still authenticates the complete
                    // source and current path. It never grants indexing access.
                    import::image_bytes(&source, &page, &self.inner.excluded, cancel)?;
                    self.check_current(&page, &source)?;
                    return Ok(Media {
                        page_id: page.id,
                        source_hash: page.source_hash,
                        data_url: Some(format!(
                            "data:image/webp;base64,{}",
                            base64::engine::general_purpose::STANDARD.encode(bytes)
                        )),
                        text: None,
                        document: None,
                    });
                }
            }
            let document = import::text(&source, &self.inner.excluded, cancel)?;
            if document.fingerprint.sha256 != page.source_hash {
                return Err(error(ErrorCode::SourceChanged));
            }
            let text = import::text_pages(&document)
                .into_iter()
                .nth(page.location.number.saturating_sub(1) as usize)
                .ok_or_else(|| error(ErrorCode::SourceChanged))?;
            self.check_current(&page, &source)?;
            return Ok(Media {
                page_id: page.id,
                source_hash: page.source_hash,
                data_url: None,
                text: Some(text),
                document: None,
            });
        }
        // A cached image never bypasses validation of the current reference.
        let validated = import::image_bytes(&source, &page, &self.inner.excluded, cancel)?;
        let edge = if thumbnail { 480 } else { 1600 };
        let bytes = match self.cache_read(&page, edge) {
            Some(bytes) => bytes,
            None => {
                let bytes = collection_render::render(&page, &validated, edge, cancel)?;
                self.check_current(&page, &source)?;
                self.cache_write(&page, edge, &bytes);
                bytes
            }
        };
        self.check_current(&page, &source)?;
        Ok(Media {
            page_id: page.id,
            source_hash: page.source_hash,
            data_url: Some(format!(
                "data:image/webp;base64,{}",
                base64::engine::general_purpose::STANDARD.encode(bytes)
            )),
            text: None,
            document: None,
        })
    }
    /// Persist the real browser-rendered first viewport, never a guessed cover.
    /// Reauthenticate the current source after rendering and re-encode bounded
    /// WebP pixels before admitting anything to our owned cache.
    pub fn cache_epub(&self, page_id: &str, source_hash: &str, data_url: &str) -> Result<Media> {
        self.cache_viewport(page_id, source_hash, data_url, PageKind::Epub)
    }
    /// Plain/extracted text uses the same browser document in the reader and
    /// thumbnail. Keep this separate from EPUB admission so one kind cannot
    /// populate another kind's cache through a mismatched IPC call.
    pub fn cache_text(&self, page_id: &str, source_hash: &str, data_url: &str) -> Result<Media> {
        self.cache_viewport(page_id, source_hash, data_url, PageKind::Text)
    }
    fn cache_viewport(
        &self,
        page_id: &str,
        source_hash: &str,
        data_url: &str,
        kind: PageKind,
    ) -> Result<Media> {
        let _guard = self
            .inner
            .render
            .lock()
            .map_err(|_| error(ErrorCode::Storage))?;
        let page = self.store.page(page_id)?;
        if page.location.kind != kind
            || page.source_hash != source_hash
            || data_url.len() > 24 << 20
        {
            return Err(error(ErrorCode::SourceChanged));
        }
        let source = self.store.source(&page.source_id)?;
        let cancel = &self.inner.shutdown;
        import::image_bytes(&source, &page, &self.inner.excluded, cancel)?;
        let payload = data_url
            .strip_prefix("data:image/webp;base64,")
            .ok_or_else(|| error(ErrorCode::Input))?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(payload)
            .map_err(|_| error(ErrorCode::Input))?;
        let (width, height) =
            image::ImageReader::with_format(std::io::Cursor::new(&bytes), image::ImageFormat::WebP)
                .into_dimensions()
                .map_err(|_| error(ErrorCode::Input))?;
        if width == 0 || height == 0 || width.max(height) > 480 {
            return Err(error(ErrorCode::Limit));
        }
        let bytes = collection_render::webp(&bytes, 480, cancel)?;
        self.check_current(&page, &source)?;
        self.cache_write(&page, 480, &bytes);
        Ok(Media {
            page_id: page.id,
            source_hash: page.source_hash,
            data_url: Some(format!(
                "data:image/webp;base64,{}",
                base64::engine::general_purpose::STANDARD.encode(bytes)
            )),
            text: None,
            document: None,
        })
    }
    fn check_current(&self, page: &Page, source: &Source) -> Result<()> {
        let now = self.store.page(&page.id)?;
        let current = self.store.source(&source.id)?;
        if now.source_hash != page.source_hash
            || current.revision != source.revision
            || current.path != source.path
        {
            return Err(error(ErrorCode::SourceChanged));
        }
        Ok(())
    }
    pub fn cover(&self, work: &str) -> Result<Media> {
        let page = self
            .store
            .first_page(work)?
            .ok_or_else(|| error(ErrorCode::NotFound))?;
        self.media(&page.id, true)
    }
}
