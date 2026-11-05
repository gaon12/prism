//! App-owned local content indexing. Filename-only scopes never reach extraction.
use crate::{
    content_store::{
        ContentChunk, ContentDocumentSummary, ContentSearchFilter, ContentSearchResponse,
        ContentStore,
    },
    document_extract::{self, ExtractOptions},
    embedding_runtime::{EmbeddingDocument, EmbeddingPhase, EmbeddingRuntime},
    settings::SearchScope,
    vector_store::{
        ChunkEmbedding, VectorDocumentBatch, VectorModelSpec, VectorSearchResponse, VectorStore,
    },
};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
use walkdir::WalkDir;

const DIMENSIONS: usize = 768;
pub(crate) struct SemanticQuery {
    vector: Vec<f32>,
    fingerprint: String,
}
const EXTRACTOR_VERSION: &str = "office-tables-v2";
fn path_key(path: &Path) -> String {
    let value = path.to_string_lossy().replace('\\', "/");
    let value = if let Some(tail) = value.strip_prefix("//?/UNC/") {
        format!("//{tail}")
    } else {
        value.strip_prefix("//?/").unwrap_or(&value).to_string()
    };
    #[cfg(windows)]
    let value = value.to_lowercase();
    value.trim_end_matches('/').to_string()
}
fn under(path: &str, root: &str) -> bool {
    path == root
        || path
            .strip_prefix(root)
            .is_some_and(|tail| tail.starts_with('/'))
}
fn is_link(path: &Path) -> bool {
    let Ok(metadata) = path.symlink_metadata() else {
        return true;
    };
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentIssue {
    pub path: String,
    pub code: String,
    pub message: String,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentStatus {
    pub phase: String,
    pub discovered: usize,
    pub indexed: usize,
    pub skipped: usize,
    pub embedded: usize,
    pub revision: u64,
    pub current_path: Option<String>,
    pub issues: Vec<ContentIssue>,
    pub embedding_error: Option<String>,
}
impl Default for ContentStatus {
    fn default() -> Self {
        Self {
            phase: "ready".into(),
            discovered: 0,
            indexed: 0,
            skipped: 0,
            embedded: 0,
            revision: 0,
            current_path: None,
            issues: vec![],
            embedding_error: None,
        }
    }
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentError {
    pub code: String,
    pub message: String,
}
fn error(code: &str, message: &str) -> ContentError {
    ContentError {
        code: code.into(),
        message: message.into(),
    }
}
type Result<T> = std::result::Result<T, ContentError>;
type Listener = Arc<dyn Fn(ContentStatus) + Send + Sync>;
struct Request {
    generation: u64,
    scopes: Vec<SearchScope>,
    cancel: Arc<AtomicBool>,
}
struct SearchTask {
    id: String,
    cancel: Arc<AtomicBool>,
}
struct Inner {
    store: ContentStore,
    vector_directory: PathBuf,
    embedding: std::result::Result<EmbeddingRuntime, String>,
    vector: Mutex<Option<(String, Arc<VectorStore>)>>,
    runtime: tokio::runtime::Runtime,
    scopes: Mutex<Vec<SearchScope>>,
    status: Mutex<ContentStatus>,
    pending: Mutex<Option<Request>>,
    scan_cancel: Mutex<Arc<AtomicBool>>,
    searches: Mutex<HashMap<String, SearchTask>>,
    generation: AtomicU64,
    closed: AtomicBool,
    paused: AtomicBool,
    listener: Listener,
    last_emit: Mutex<Instant>,
    excluded: Mutex<Vec<String>>,
    dirty: AtomicBool,
    watch_issues: Mutex<Vec<ContentIssue>>,
}
#[derive(Clone)]
pub struct ContentService {
    inner: Arc<Inner>,
}
impl ContentService {
    pub fn open(
        directory: &Path,
        embedding: std::result::Result<EmbeddingRuntime, String>,
        listener: impl Fn(ContentStatus) + Send + Sync + 'static,
    ) -> Result<Self> {
        let service = Self {
            inner: Arc::new(Inner {
                store: ContentStore::open(&directory.join("content.sqlite3"))
                    .map_err(|_| error("storage", "Could not open the local content index."))?,
                vector_directory: directory.join("vectors"),
                embedding,
                vector: Mutex::new(None),
                runtime: tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                    .map_err(|_| error("worker", "Could not create the local search worker."))?,
                scopes: Mutex::new(vec![]),
                status: Mutex::new(ContentStatus::default()),
                pending: Mutex::new(None),
                scan_cancel: Mutex::new(Arc::new(AtomicBool::new(false))),
                searches: Mutex::new(HashMap::new()),
                generation: AtomicU64::new(0),
                closed: AtomicBool::new(false),
                paused: AtomicBool::new(false),
                listener: Arc::new(listener),
                last_emit: Mutex::new(Instant::now() - Duration::from_secs(1)),
                excluded: Mutex::new(vec![path_key(
                    &std::path::absolute(directory).unwrap_or_else(|_| directory.to_path_buf()),
                )]),
                dirty: AtomicBool::new(false),
                watch_issues: Mutex::new(vec![]),
            }),
        };
        let worker = service.clone();
        thread::spawn(move || worker.monitor());
        Ok(service)
    }
    pub fn status(&self) -> Result<ContentStatus> {
        self.inner
            .status
            .lock()
            .map(|status| status.clone())
            .map_err(|_| error("worker", "Content status unavailable."))
    }
    /// Internal app caches must never become a source or trigger indexing feedback loops.
    pub fn exclude_paths(&self, paths: &[PathBuf]) {
        if let Ok(mut excluded) = self.inner.excluded.lock() {
            excluded.extend(
                paths
                    .iter()
                    .filter_map(|path| std::path::absolute(path).ok())
                    .map(|path| path_key(&path)),
            );
            excluded.sort();
            excluded.dedup();
        }
    }
    fn is_excluded(&self, path: &str) -> bool {
        self.inner
            .excluded
            .lock()
            .map(|roots| {
                roots
                    .iter()
                    .any(|root| under(&path_key(Path::new(path)), root))
            })
            .unwrap_or(true)
    }
    fn update(&self, action: impl FnOnce(&mut ContentStatus)) {
        if let Ok(mut status) = self.inner.status.lock() {
            let previous_phase = status.phase.clone();
            action(&mut status);
            status.revision = status.revision.wrapping_add(1);
            let snapshot = status.clone();
            drop(status);
            let force = previous_phase != snapshot.phase || snapshot.phase != "indexing";
            if let Ok(mut emitted) = self.inner.last_emit.lock() {
                if force || emitted.elapsed() >= Duration::from_millis(150) {
                    *emitted = Instant::now();
                    drop(emitted);
                    (self.inner.listener)(snapshot);
                }
            }
        }
    }
    pub fn configure(&self, scopes: Vec<SearchScope>) -> Result<()> {
        self.inner.paused.store(false, Ordering::Release);
        let mut current_scopes = self
            .inner
            .scopes
            .lock()
            .map_err(|_| error("worker", "Content scopes unavailable."))?;
        let changed = current_scopes.len() != scopes.len()
            || current_scopes.iter().zip(&scopes).any(|(left, right)| {
                left.id != right.id
                    || left.path != right.path
                    || left.content_enabled != right.content_enabled
            });
        *current_scopes = scopes.clone();
        drop(current_scopes);
        let generation = self.inner.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let cancel = Arc::new(AtomicBool::new(false));
        if let Ok(mut previous) = self.inner.scan_cancel.lock() {
            previous.store(true, Ordering::Release);
            *previous = cancel.clone();
        }
        *self
            .inner
            .pending
            .lock()
            .map_err(|_| error("worker", "Content scheduler unavailable."))? = Some(Request {
            generation,
            scopes,
            cancel,
        });
        if let Ok(searches) = self.inner.searches.lock() {
            if changed {
                for task in searches.values() {
                    task.cancel.store(true, Ordering::Release);
                }
            }
        }
        self.update(|status| {
            status.phase = "indexing".into();
            status.current_path = None;
        });
        Ok(())
    }
    pub fn refresh(&self) -> Result<()> {
        let scopes = self
            .inner
            .scopes
            .lock()
            .map_err(|_| error("worker", "Content scopes unavailable."))?
            .clone();
        self.configure(scopes)
    }
    pub fn cancel_index(&self) {
        self.inner.paused.store(true, Ordering::Release);
        if let Ok(flag) = self.inner.scan_cancel.lock() {
            flag.store(true, Ordering::Release);
        }
        if let Ok(mut pending) = self.inner.pending.lock() {
            *pending = None;
        }
        self.update(|status| {
            status.phase = "cancelled".into();
            status.current_path = None;
        });
    }
    pub fn shutdown(&self) {
        self.inner.closed.store(true, Ordering::Release);
        self.cancel_index();
        if let Ok(searches) = self.inner.searches.lock() {
            for search in searches.values() {
                search.cancel.store(true, Ordering::Release);
            }
        }
    }
    fn monitor(&self) {
        let mut last_scan = Instant::now();
        let mut watcher: Option<RecommendedWatcher> = None;
        let mut watched_roots = Vec::new();
        let mut last_watch_attempt = Instant::now() - Duration::from_secs(60);
        loop {
            if self.inner.closed.load(Ordering::Acquire) || Arc::strong_count(&self.inner) == 1 {
                return;
            }
            let request = self
                .inner
                .pending
                .lock()
                .ok()
                .and_then(|mut request| request.take());
            if let Some(request) = request {
                let roots = Self::roots(&request.scopes, None);
                let watch_failed = self
                    .inner
                    .watch_issues
                    .lock()
                    .map(|issues| !issues.is_empty())
                    .unwrap_or(true);
                if roots != watched_roots
                    || (last_watch_attempt.elapsed() >= Duration::from_secs(60)
                        && (watcher.is_none() || watch_failed))
                {
                    watcher = self.watch(&roots);
                    watched_roots = roots;
                    last_watch_attempt = Instant::now();
                }
                self.inner.dirty.store(false, Ordering::Release);
                self.scan(request);
                last_scan = Instant::now();
            } else if self.inner.generation.load(Ordering::Acquire) > 0
                && !self.inner.paused.load(Ordering::Acquire)
                && (last_scan.elapsed() >= Duration::from_secs(60)
                    || (self.inner.dirty.load(Ordering::Acquire)
                        && last_scan.elapsed() >= Duration::from_millis(750)))
            {
                let _ = self.refresh();
                last_scan = Instant::now();
            }
            thread::sleep(Duration::from_millis(250));
        }
    }
    fn watch(&self, roots: &[PathBuf]) -> Option<RecommendedWatcher> {
        if let Ok(mut issues) = self.inner.watch_issues.lock() {
            issues.clear();
        }
        if roots.is_empty() {
            return None;
        }
        let weak = Arc::downgrade(&self.inner);
        let mut watcher =
            match notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                let Some(inner) = weak.upgrade() else {
                    return;
                };
                match event {
                    Ok(event) if !matches!(event.kind, EventKind::Access(_)) => {
                        let excluded = inner
                            .excluded
                            .lock()
                            .map(|paths| paths.clone())
                            .unwrap_or_default();
                        if event.paths.iter().any(|path| {
                            !excluded.iter().any(|root| under(&path_key(path), root))
                                && (path.is_dir()
                                    || !path.exists()
                                    || document_extract::is_supported_extension(
                                        path.extension()
                                            .and_then(|value| value.to_str())
                                            .unwrap_or(""),
                                    ))
                        }) {
                            inner.dirty.store(true, Ordering::Release);
                        }
                    }
                    Err(_) => {
                        inner.dirty.store(true, Ordering::Release);
                    }
                    _ => {}
                }
            }) {
                Ok(watcher) => watcher,
                Err(_) => {
                    self.record_watch_issue(Path::new(""));
                    return None;
                }
            };
        for root in roots {
            if watcher.watch(root, RecursiveMode::Recursive).is_err() {
                self.record_watch_issue(root);
            }
        }
        Some(watcher)
    }
    fn record_watch_issue(&self, root: &Path) {
        if let Ok(mut issues) = self.inner.watch_issues.lock() {
            if issues.len() < 30 {
                issues.push(ContentIssue {
                    path: root.to_string_lossy().into(), code: "watcher".into(),
                    message: "Live monitoring is unavailable for this folder. Periodic reconciliation will retry.".into(),
                });
            }
        }
    }
    fn current(&self, request: &Request) -> bool {
        !request.cancel.load(Ordering::Acquire)
            && !self.inner.closed.load(Ordering::Acquire)
            && self.inner.generation.load(Ordering::Acquire) == request.generation
            && Arc::strong_count(&self.inner) > 1
    }
    fn roots(scopes: &[SearchScope], scope_id: Option<&str>) -> Vec<PathBuf> {
        let mut roots: Vec<PathBuf> = scopes
            .iter()
            .filter(|scope| scope.content_enabled && scope_id.is_none_or(|id| scope.id == id))
            .filter_map(|scope| fs::canonicalize(&scope.path).ok())
            .filter(|path| path.is_dir())
            .collect();
        roots.sort();
        roots.dedup();
        roots
    }
    fn active_roots(&self, scope_id: Option<&str>) -> Result<Vec<PathBuf>> {
        let scopes = self
            .inner
            .scopes
            .lock()
            .map_err(|_| error("worker", "Content scopes unavailable."))?;
        Ok(Self::roots(&scopes, scope_id))
    }
    fn selected_roots(&self, ids: &[String]) -> Result<Vec<PathBuf>> {
        let scopes = self
            .inner
            .scopes
            .lock()
            .map_err(|_| error("worker", "Content scopes unavailable."))?;
        let selected: Vec<_> = scopes
            .iter()
            .filter(|scope| ids.is_empty() || ids.contains(&scope.id))
            .cloned()
            .collect();
        Ok(Self::roots(&selected, None))
    }
    fn issue(&self, path: &Path, code: &str, message: &str) {
        self.update(|status| {
            status.skipped += 1;
            if status.issues.len() < 30 {
                status.issues.push(ContentIssue {
                    path: path.to_string_lossy().into(),
                    code: code.into(),
                    message: message.into(),
                });
            }
        });
    }
    fn scan(&self, request: Request) {
        if !self.current(&request) {
            return;
        }
        let roots = Self::roots(&request.scopes, None);
        let unavailable: Vec<PathBuf> = request
            .scopes
            .iter()
            .filter(|scope| scope.content_enabled && !Path::new(&scope.path).is_dir())
            .filter_map(|scope| std::path::absolute(&scope.path).ok())
            .collect();
        self.update(|status| {
            status.phase = "indexing".into();
            status.discovered = 0;
            status.skipped = 0;
            status.embedded = 0;
            status.indexed = 0;
            status.issues = self
                .inner
                .watch_issues
                .lock()
                .map(|issues| issues.clone())
                .unwrap_or_default();
            status.embedding_error = None;
        });
        for scope in request.scopes.iter().filter(|scope| scope.content_enabled) {
            if !Path::new(&scope.path).is_dir() {
                self.issue(
                    Path::new(&scope.path),
                    "io",
                    "Content folder is unavailable.",
                );
            }
        }
        if let Err(error) = self
            .inner
            .store
            .purge_for_scopes(&roots, &unavailable, &request.cancel)
        {
            if self.current(&request) {
                self.update(|status| {
                    status.phase = "error".into();
                    status.embedding_error = Some(error.message);
                });
            }
            return;
        }
        let excluded = self
            .inner
            .excluded
            .lock()
            .map(|paths| paths.clone())
            .unwrap_or_default();
        for path in &excluded {
            if !self.current(&request) {
                return;
            }
            let _ = self
                .inner
                .store
                .purge_scope(Path::new(path), &request.cancel);
        }
        let vector = if self
            .inner
            .embedding
            .as_ref()
            .is_ok_and(|embedding| embedding.status().phase == EmbeddingPhase::Running)
        {
            match self.vector() {
                Ok(vector) => Some(vector),
                Err(error) => {
                    if !self.current(&request) {
                        return;
                    }
                    self.update(|status| status.embedding_error = Some(error.message));
                    None
                }
            }
        } else {
            None
        };
        // Search revalidates all sources. Defer physical vector cleanup while a
        // configured drive is offline so reconnecting does not require re-embedding.
        if let Some(vector) = vector.as_ref().filter(|_| unavailable.is_empty()) {
            if let Err(error) = self.inner.runtime.block_on(vector.purge_outside_roots(
                &self.inner.store,
                &roots,
                &request.cancel,
            )) {
                self.update(|status| status.embedding_error = Some(error.message));
            }
        }
        let mut seen = HashSet::new();
        let excluded = self
            .inner
            .excluded
            .lock()
            .map(|paths| paths.clone())
            .unwrap_or_default();
        for root in &roots {
            for entry in WalkDir::new(root)
                .follow_links(false)
                .into_iter()
                .filter_entry(|entry| {
                    !is_link(entry.path())
                        && !excluded
                            .iter()
                            .any(|root| under(&path_key(entry.path()), root))
                })
            {
                if !self.current(&request) {
                    return;
                }
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(_) => {
                        self.issue(root, "io", "Some content paths could not be read.");
                        continue;
                    }
                };
                if !entry.file_type().is_file()
                    || entry
                        .path()
                        .symlink_metadata()
                        .is_ok_and(|metadata| metadata.file_type().is_symlink())
                {
                    continue;
                }
                let path = entry.path();
                if !document_extract::is_supported_extension(
                    path.extension()
                        .and_then(|extension| extension.to_str())
                        .unwrap_or(""),
                ) {
                    continue;
                }
                if !seen.insert(path.to_path_buf()) {
                    continue;
                }
                // Authorize before reading the document, not just before storing its text.
                let canonical = match fs::canonicalize(path) {
                    Ok(canonical)
                        if canonical.is_file()
                            && roots.iter().any(|root| canonical.starts_with(root)) =>
                    {
                        canonical
                    }
                    _ => {
                        self.issue(
                            path,
                            "unsafePath",
                            "Document is outside the active content folders.",
                        );
                        continue;
                    }
                };
                let path = canonical.as_path();
                self.update(|status| {
                    status.discovered += 1;
                    status.current_path = Some(path.to_string_lossy().into());
                });
                let previous = self
                    .inner
                    .store
                    .document_summary(path, &roots, &request.cancel)
                    .ok()
                    .flatten()
                    .filter(|document| {
                        document.extractor_version.as_deref() == Some(EXTRACTOR_VERSION)
                    });
                let document = if let Some(previous) = previous {
                    previous
                } else {
                    let extracted = match document_extract::extract_document(
                        path,
                        &ExtractOptions::default(),
                        &request.cancel,
                    ) {
                        Ok(extracted) => extracted,
                        Err(error) => {
                            if !self.current(&request) {
                                return;
                            }
                            let code = serde_json::to_value(error.code)
                                .ok()
                                .and_then(|value| value.as_str().map(String::from))
                                .unwrap_or_else(|| "extract".into());
                            self.issue(path, &code, &error.message);
                            continue;
                        }
                    };
                    if !self.current(&request) {
                        return;
                    }
                    match self
                        .inner
                        .store
                        .replace_document(&extracted, &roots, &request.cancel)
                    {
                        Ok(document) => document,
                        Err(error) => {
                            if !self.current(&request) {
                                return;
                            }
                            let code = serde_json::to_value(error.code)
                                .ok()
                                .and_then(|value| value.as_str().map(String::from))
                                .unwrap_or_else(|| "storage".into());
                            self.issue(path, &code, &error.message);
                            continue;
                        }
                    }
                };
                if document.extractor_version.as_deref() != Some(EXTRACTOR_VERSION) {
                    if let Err(error) = self.inner.store.mark_extraction_version(
                        &document,
                        EXTRACTOR_VERSION,
                        &roots,
                        &request.cancel,
                    ) {
                        if !self.current(&request) {
                            return;
                        }
                        self.issue(path, "storage", &error.message);
                        continue;
                    }
                }
                if let Some(vector) = &vector {
                    if let Err(error) =
                        self.embed_document(&document, vector, &roots, &request.cancel)
                    {
                        if self.current(&request) {
                            self.update(|status| status.embedding_error = Some(error.message));
                        }
                    } else {
                        self.update(|status| status.embedded += 1);
                    }
                }
                self.update(|status| status.indexed += 1);
            }
        }
        if !self.current(&request) {
            return;
        }
        self.update(|status| {
            status.current_path = None;
            status.phase = "ready".into();
        });
    }
    fn vector(&self) -> Result<Arc<VectorStore>> {
        let embedding = self
            .inner
            .embedding
            .as_ref()
            .map_err(|message| error("model", message))?;
        let fingerprint = embedding
            .fingerprint(DIMENSIONS)
            .map_err(|message| error("model", &message))?;
        let mut cache = self
            .inner
            .vector
            .lock()
            .map_err(|_| error("worker", "Vector index unavailable."))?;
        if let Some((key, vector)) = &*cache {
            if key == &fingerprint {
                return Ok(vector.clone());
            }
        }
        let model = VectorModelSpec {
            model_fingerprint: fingerprint.clone(),
            dimensions: DIMENSIONS,
            chunking_version: format!("sections-1200-overlap150-{EXTRACTOR_VERSION}"),
        };
        let vector = Arc::new(
            self.inner
                .runtime
                .block_on(VectorStore::open(&self.inner.vector_directory, model))
                .map_err(|error| ContentError {
                    code: "vector".into(),
                    message: error.message,
                })?,
        );
        *cache = Some((fingerprint, vector.clone()));
        Ok(vector)
    }
    fn embed_document(
        &self,
        document: &ContentDocumentSummary,
        vector: &VectorStore,
        roots: &[PathBuf],
        cancel: &AtomicBool,
    ) -> Result<()> {
        let chunks = self
            .inner
            .store
            .chunks_for_document(Path::new(&document.file_path), roots, cancel)
            .map_err(|error| ContentError {
                code: "source".into(),
                message: error.message,
            })?;
        let ids: Vec<_> = chunks.iter().map(|chunk| chunk.id.clone()).collect();
        if self
            .inner
            .runtime
            .block_on(vector.has_current_document(
                Path::new(&document.file_path),
                &document.fingerprint,
                &ids,
                &self.inner.store,
                roots,
                cancel,
            ))
            .map_err(|error| ContentError {
                code: "vector".into(),
                message: error.message,
            })?
        {
            return Ok(());
        }
        let embedding = self
            .inner
            .embedding
            .as_ref()
            .map_err(|message| error("model", message))?;
        let mut embeddings = Vec::with_capacity(chunks.len());
        for batch in chunks.chunks(32) {
            if cancel.load(Ordering::Acquire) {
                return Err(error("cancelled", "Content indexing cancelled."));
            }
            let input: Vec<_> = batch
                .iter()
                .map(|chunk| EmbeddingDocument {
                    title: Some(
                        Path::new(&chunk.file_path)
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into(),
                    ),
                    text: chunk.text.clone(),
                })
                .collect();
            let vectors = embedding
                .embed_documents(&input, DIMENSIONS)
                .map_err(|message| error("model", &message))?;
            embeddings.extend(
                batch
                    .iter()
                    .zip(vectors)
                    .map(|(chunk, vector)| ChunkEmbedding {
                        chunk_id: chunk.id.clone(),
                        vector,
                    }),
            );
        }
        let batch = VectorDocumentBatch {
            file_path: document.file_path.clone(),
            fingerprint: document.fingerprint.clone(),
            model_fingerprint: embedding
                .fingerprint(DIMENSIONS)
                .map_err(|message| error("model", &message))?,
            embeddings,
        };
        self.inner
            .runtime
            .block_on(vector.replace_document(batch, &self.inner.store, roots, cancel))
            .map_err(|error| ContentError {
                code: "vector".into(),
                message: error.message,
            })?;
        Ok(())
    }
    pub fn begin_search(&self, window: &str, id: &str) -> Result<Arc<AtomicBool>> {
        if id.is_empty() || id.len() > 80 {
            return Err(error("input", "Invalid search request identifier."));
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let previous = self
            .inner
            .searches
            .lock()
            .map_err(|_| error("worker", "Search worker unavailable."))?
            .insert(
                window.into(),
                SearchTask {
                    id: id.into(),
                    cancel: cancel.clone(),
                },
            );
        if let Some(previous) = previous {
            previous.cancel.store(true, Ordering::Release);
        }
        Ok(cancel)
    }
    pub fn cancel_search(&self, window: &str, id: &str) {
        if let Ok(mut searches) = self.inner.searches.lock() {
            if searches.get(window).is_some_and(|task| task.id == id) {
                if let Some(task) = searches.remove(window) {
                    task.cancel.store(true, Ordering::Release);
                }
            }
        }
    }
    fn search_filter(&self, extension: &str) -> Result<ContentSearchFilter> {
        let values = if extension.trim().is_empty() {
            Vec::new()
        } else {
            extension
                .split([',', ';', ' '])
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(String::from)
                .collect()
        };
        let extensions =
            crate::content_store::normalized_extensions(&values).map_err(|error| ContentError {
                code: "input".into(),
                message: error.message,
            })?;
        Ok(ContentSearchFilter {
            extensions,
            ..Default::default()
        })
    }
    pub fn search(
        &self,
        query: &str,
        extension: &str,
        scope_id: Option<&str>,
        cancel: &AtomicBool,
    ) -> Result<ContentSearchResponse> {
        self.search_in_scopes(
            query,
            extension,
            &scope_id.map(String::from).into_iter().collect::<Vec<_>>(),
            cancel,
        )
    }
    pub(crate) fn search_in_scopes(
        &self,
        query: &str,
        extension: &str,
        scope_ids: &[String],
        cancel: &AtomicBool,
    ) -> Result<ContentSearchResponse> {
        self.search_selected(query, extension, scope_ids, None, cancel)
    }
    pub(crate) fn search_selected(
        &self,
        query: &str,
        extension: &str,
        scope_ids: &[String],
        documents: Option<&[String]>,
        cancel: &AtomicBool,
    ) -> Result<ContentSearchResponse> {
        let roots = self.selected_roots(scope_ids)?;
        let mut filter = self.search_filter(extension)?;
        if let Some(paths) = documents {
            filter.document_paths = paths.to_vec();
        }
        if documents.is_some_and(<[String]>::is_empty) {
            return Ok(ContentSearchResponse {
                hits: vec![],
                candidate_count: 0,
                candidate_limit: 100,
                hard_limit_reached: false,
            });
        }
        let mut response = self
            .inner
            .store
            .search_detailed(query, &filter, &roots, 100, cancel)
            .map_err(|error| ContentError {
                code: serde_json::to_value(error.code)
                    .ok()
                    .and_then(|value| value.as_str().map(String::from))
                    .unwrap_or_else(|| "source".into()),
                message: error.message,
            })?;
        response
            .hits
            .retain(|hit| !self.is_excluded(&hit.chunk.file_path));
        Ok(response)
    }
    pub fn semantic_search(
        &self,
        query: &str,
        extension: &str,
        scope_id: Option<&str>,
        cancel: &AtomicBool,
    ) -> Result<VectorSearchResponse> {
        self.semantic_search_in_scopes(
            query,
            extension,
            &scope_id.map(String::from).into_iter().collect::<Vec<_>>(),
            20,
            cancel,
        )
    }
    pub(crate) fn semantic_search_in_scopes(
        &self,
        query: &str,
        extension: &str,
        scope_ids: &[String],
        limit: usize,
        cancel: &AtomicBool,
    ) -> Result<VectorSearchResponse> {
        self.semantic_search_selected(query, extension, scope_ids, limit, None, cancel)
    }
    pub(crate) fn semantic_search_selected(
        &self,
        query: &str,
        extension: &str,
        scope_ids: &[String],
        limit: usize,
        documents: Option<&[String]>,
        cancel: &AtomicBool,
    ) -> Result<VectorSearchResponse> {
        if !(1..=128).contains(&limit) {
            return Err(error("input", "Invalid semantic candidate budget."));
        }
        if query.trim().is_empty() || query.len() > 4096 {
            return Err(error(
                "input",
                "Enter a natural-language search up to 4096 bytes.",
            ));
        }
        let mut filter = self.search_filter(extension)?;
        if let Some(paths) = documents {
            filter.document_paths = paths.to_vec();
        }
        let prepared = self.prepare_semantic_query(query, cancel)?;
        self.run_semantic_query(&prepared, &filter, scope_ids, limit, cancel)
    }
    pub(crate) fn prepare_semantic_query(
        &self,
        query: &str,
        cancel: &AtomicBool,
    ) -> Result<SemanticQuery> {
        if query.trim().is_empty() || query.len() > 4096 {
            return Err(error("input", "Enter a search up to 4096 bytes."));
        }
        let embedding = self
            .inner
            .embedding
            .as_ref()
            .map_err(|message| error("model", message))?;
        if embedding.status().phase != EmbeddingPhase::Running {
            return Err(error(
                "modelNotRunning",
                "Start the local embedding model to use natural-language search.",
            ));
        }
        if cancel.load(Ordering::Acquire) {
            return Err(error("cancelled", "Search cancelled."));
        }
        let query_vector = embedding
            .embed_query(query, DIMENSIONS)
            .map_err(|message| error("model", &message))?;
        let fingerprint = embedding
            .fingerprint(DIMENSIONS)
            .map_err(|message| error("model", &message))?;
        Ok(SemanticQuery {
            vector: query_vector,
            fingerprint,
        })
    }
    pub(crate) fn run_semantic_query(
        &self,
        query: &SemanticQuery,
        filter: &ContentSearchFilter,
        scope_ids: &[String],
        limit: usize,
        cancel: &AtomicBool,
    ) -> Result<VectorSearchResponse> {
        if !(1..=128).contains(&limit)
            || !filter.document_paths.is_empty() && filter.document_paths.len() > 256
        {
            return Err(error("input", "Invalid semantic candidate budget."));
        }
        let roots = self.selected_roots(scope_ids)?;
        let vector = self.vector()?;
        let mut response = self
            .inner
            .runtime
            .block_on(vector.search(
                &query.vector,
                &query.fingerprint,
                filter,
                &self.inner.store,
                &roots,
                limit,
                4096,
                cancel,
            ))
            .map_err(|error| ContentError {
                code: "vector".into(),
                message: error.message,
            })?;
        let before = response.hits.len();
        response
            .hits
            .retain(|hit| !self.is_excluded(&hit.chunk.file_path));
        response.filtered_out += before - response.hits.len();
        Ok(response)
    }
    pub fn automatic_search(
        &self,
        query: &str,
        extension: &str,
        scope_id: Option<&str>,
        semantic_enabled: bool,
        hybrid: bool,
        cancel: &AtomicBool,
    ) -> Result<crate::automatic_search::AutomaticResponse> {
        use crate::automatic_search::{AutomaticResponse, SemanticState};
        if cancel.load(Ordering::Acquire) {
            return Err(error("cancelled", "Search cancelled."));
        }
        if query.trim().is_empty() || query.len() > 4096 {
            return Err(error("input", "Enter a search up to 4096 bytes."));
        }
        let running = semantic_enabled
            && self
                .inner
                .embedding
                .as_ref()
                .is_ok_and(|model| model.status().phase == EmbeddingPhase::Running);
        let mut semantic_state = if semantic_enabled {
            SemanticState::NotRunning
        } else {
            SemanticState::Disabled
        };
        let mut failure = None;
        if running {
            let result = if hybrid {
                self.hybrid_search(query, extension, scope_id, cancel)
                    .map(AutomaticResponse::hybrid)
            } else {
                self.semantic_search(query, extension, scope_id, cancel)
                    .map(AutomaticResponse::semantic)
            };
            match result {
                Ok(value) => return Ok(value),
                Err(value)
                    if !crate::automatic_search::can_fallback_after_semantic_error(&value.code) =>
                {
                    return Err(value)
                }
                Err(value) => {
                    semantic_state = SemanticState::Failed;
                    failure = Some(value.code);
                }
            }
        }
        let mut skipped = false;
        let keyword = match self.search(query, extension, scope_id, cancel) {
            Ok(value) => value,
            Err(value) if value.code.eq_ignore_ascii_case("invalidInput") => {
                skipped = true;
                ContentSearchResponse {
                    hits: Vec::new(),
                    candidate_count: 0,
                    candidate_limit: 0,
                    hard_limit_reached: false,
                }
            }
            Err(value) => return Err(value),
        };
        if cancel.load(Ordering::Acquire) {
            return Err(error("cancelled", "Search cancelled."));
        }
        let mut result = AutomaticResponse::keyword(keyword, semantic_state, failure);
        result.keyword_skipped = skipped;
        Ok(result)
    }
    pub fn hybrid_search(
        &self,
        query: &str,
        extension: &str,
        scope_id: Option<&str>,
        cancel: &AtomicBool,
    ) -> Result<crate::hybrid_search::HybridResponse> {
        let ids: Vec<_> = scope_id.map(String::from).into_iter().collect();
        let semantic = self.semantic_search_in_scopes(query, extension, &ids, 100, cancel)?;
        let keyword = match self.search_in_scopes(query, extension, &ids, cancel) {
            Ok(value) => Some(value),
            // Natural language can exceed the lexical branch's 32-term budget.
            // Preserve semantic search and explicitly report the skipped branch.
            Err(error) if error.code.eq_ignore_ascii_case("invalidInput") => None,
            Err(error) => return Err(error),
        };
        let mut result = crate::hybrid_search::fuse(keyword, semantic, 100);
        if cancel.load(Ordering::Acquire) {
            return Err(error("cancelled", "Search cancelled."));
        }
        let requested_ids: Vec<_> = result.hits.iter().map(|hit| hit.chunk.id.clone()).collect();
        let current = self
            .inner
            .store
            .chunks_by_ids(&requested_ids, &self.selected_roots(&ids)?, cancel)
            .map_err(|value| {
                if cancel.load(Ordering::Acquire) {
                    error("cancelled", "Search cancelled.")
                } else {
                    error("source", &value.message)
                }
            })?;
        let current: HashMap<_, _> = current
            .into_iter()
            .map(|chunk| (chunk.id.clone(), chunk))
            .collect();
        result.hits.retain(|hit| {
            !self.is_excluded(&hit.chunk.file_path)
                && current.get(&hit.chunk.id).is_some_and(|chunk| {
                    chunk.file_path == hit.chunk.file_path
                        && chunk.fingerprint == hit.chunk.fingerprint
                })
        });
        if cancel.load(Ordering::Acquire) {
            return Err(error("cancelled", "Search cancelled."));
        }
        Ok(result)
    }
    pub fn chunks(&self, ids: &[String], cancel: &AtomicBool) -> Result<Vec<ContentChunk>> {
        let mut chunks = self
            .inner
            .store
            .chunks_by_ids(ids, &self.active_roots(None)?, cancel)
            .map_err(|error| ContentError {
                code: "source".into(),
                message: error.message,
            })?;
        chunks.retain(|chunk| !self.is_excluded(&chunk.file_path));
        Ok(chunks)
    }
    pub fn resolve_document(&self, path: &Path) -> Result<PathBuf> {
        if self.is_excluded(&path.to_string_lossy()) {
            return Err(error(
                "source",
                "App cache files cannot be used as document sources.",
            ));
        }
        self.inner
            .store
            .document_summary(path, &self.active_roots(None)?, &AtomicBool::new(false))
            .map_err(|_| error("source", "Content source is unavailable."))?
            .map(|document| PathBuf::from(document.file_path))
            .ok_or_else(|| {
                error(
                    "source",
                    "This document is no longer a current authorized source.",
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn automatic_without_a_running_model_keeps_authorized_keywords_and_explicit_limits() {
        use crate::automatic_search::{AutomaticRanking, SemanticState};
        let root = tempfile::tempdir().unwrap();
        let docs = root.path().join("docs");
        fs::create_dir(&docs).unwrap();
        fs::write(docs.join("guide.md"), "본문토큰 자료입니다.").unwrap();
        let service = ContentService::open(
            &root.path().join("data"),
            Err("Synthetic model unavailable".into()),
            |_| {},
        )
        .unwrap();
        service
            .configure(vec![SearchScope {
                id: "docs".into(),
                name: "docs".into(),
                path: docs.to_string_lossy().into(),
                filename_enabled: false,
                content_enabled: true,
                external_allowed: false,
            }])
            .unwrap();
        wait_ready(&service);
        let cancel = AtomicBool::new(false);
        let value = service
            .automatic_search("본문토큰", "", Some("docs"), true, true, &cancel)
            .unwrap();
        assert_eq!(value.ranking, AutomaticRanking::Keyword);
        assert_eq!(value.semantic_state, SemanticState::NotRunning);
        assert!(!value.keyword_skipped);
        assert!(!value.hits.is_empty());
        let paths = vec![docs.join("guide.md").to_string_lossy().into_owned()];
        assert!(!service
            .search_selected("본문토큰", "", &["docs".into()], Some(&paths), &cancel)
            .unwrap()
            .hits
            .is_empty());
        assert!(service
            .search_selected("본문토큰", "", &["docs".into()], Some(&[]), &cancel)
            .unwrap()
            .hits
            .is_empty());
        assert!(service
            .search_selected(
                "본문토큰",
                "",
                &["docs".into()],
                Some(&["missing.md".into()]),
                &cancel
            )
            .unwrap()
            .hits
            .is_empty());
        assert!(!service
            .automatic_search("본문토큰", "pdf,.MD", Some("docs"), false, true, &cancel)
            .unwrap()
            .hits
            .is_empty());
        assert!(service
            .automatic_search("본문토큰", "pdf; txt", Some("docs"), false, true, &cancel)
            .unwrap()
            .hits
            .is_empty());
        assert_eq!(
            service
                .automatic_search("본문토큰", "", Some("docs"), false, true, &cancel)
                .unwrap()
                .semantic_state,
            SemanticState::Disabled
        );
        assert!(service
            .automatic_search("본문토큰", "", Some("other"), true, true, &cancel)
            .unwrap()
            .hits
            .is_empty());
        let long = (0..33)
            .map(|index| format!("term{index}"))
            .collect::<Vec<_>>()
            .join(" ");
        let value = service
            .automatic_search(&long, "", Some("docs"), true, true, &cancel)
            .unwrap();
        assert!(value.keyword_skipped);
        assert_eq!(value.ranking, AutomaticRanking::Keyword);
        assert!(value.hits.is_empty());
        assert!(service
            .automatic_search("본문토큰", "../md", None, true, true, &cancel)
            .is_err());
        assert!(service
            .automatic_search("", "", None, true, true, &cancel)
            .is_err());
        cancel.store(true, Ordering::Release);
        assert_eq!(
            service
                .automatic_search("본문토큰", "", None, true, true, &cancel)
                .unwrap_err()
                .code,
            "cancelled"
        );
    }
    fn wait_ready(service: &ContentService) {
        let until = Instant::now() + Duration::from_secs(10);
        while Instant::now() < until {
            let status = service.status().unwrap();
            if status.phase == "ready" {
                return;
            }
            assert_ne!(status.phase, "error", "Index fixture must not fail");
            thread::sleep(Duration::from_millis(20));
        }
        panic!("Content fixture did not finish indexing");
    }
    #[test]
    fn content_scopes_are_independent_and_current_sources_survive_restart() {
        let root = tempfile::tempdir().unwrap();
        let documents = root.path().join("문서");
        fs::create_dir(&documents).unwrap();
        let source = documents.join("여행.md");
        fs::write(&source, "# 출장\n제주도 출장 경비는 십만원입니다.").unwrap();
        let service = ContentService::open(
            &root.path().join("index"),
            Err("Synthetic model not configured".into()),
            |_| {},
        )
        .unwrap();
        let mut scope = SearchScope {
            id: "body".into(),
            name: "문서".into(),
            path: documents.to_string_lossy().into(),
            filename_enabled: true,
            content_enabled: false,
            external_allowed: false,
        };
        service.configure(vec![scope.clone()]).unwrap();
        wait_ready(&service);
        assert!(service
            .search("제주도", "", None, &AtomicBool::new(false))
            .unwrap()
            .hits
            .is_empty());
        scope.filename_enabled = false;
        scope.content_enabled = true;
        service.configure(vec![scope.clone()]).unwrap();
        wait_ready(&service);
        let hits = service
            .search("제주도 경비", "md", Some("body"), &AtomicBool::new(false))
            .unwrap();
        assert_eq!(hits.hits.len(), 1);
        let id = hits.hits[0].chunk.id.clone();
        assert_eq!(
            service
                .chunks(&[id.clone()], &AtomicBool::new(false))
                .unwrap()
                .len(),
            1
        );
        let revision = service.status().unwrap().revision;
        service.refresh().unwrap();
        wait_ready(&service);
        assert!(service.status().unwrap().revision > revision);
        assert_eq!(
            service
                .search("제주도", "", None, &AtomicBool::new(false))
                .unwrap()
                .hits[0]
                .chunk
                .id,
            id
        );
        service.shutdown();
        drop(service);
        let service = ContentService::open(
            &root.path().join("index"),
            Err("Synthetic model not configured".into()),
            |_| {},
        )
        .unwrap();
        service.configure(vec![scope]).unwrap();
        wait_ready(&service);
        assert_eq!(
            service
                .search("제주도", "", None, &AtomicBool::new(false))
                .unwrap()
                .hits[0]
                .chunk
                .id,
            id
        );
        service.configure(vec![]).unwrap();
        assert!(
            service
                .search("제주도", "", None, &AtomicBool::new(false))
                .unwrap()
                .hits
                .is_empty(),
            "Removed scope must disappear before the next scan"
        );
        assert!(service
            .chunks(&[id], &AtomicBool::new(false))
            .unwrap()
            .is_empty());
        service.shutdown();
    }
    #[test]
    fn a_new_search_cancels_only_the_previous_request_in_its_window() {
        let root = tempfile::tempdir().unwrap();
        let service = ContentService::open(
            root.path(),
            Err("Synthetic model not configured".into()),
            |_| {},
        )
        .unwrap();
        let old = service.begin_search("main", "old").unwrap();
        let quick = service.begin_search("quick", "other-window").unwrap();
        let new = service.begin_search("main", "new").unwrap();
        assert!(old.load(Ordering::Acquire));
        assert!(!quick.load(Ordering::Acquire));
        service.refresh().unwrap();
        assert!(
            !new.load(Ordering::Acquire),
            "Reconciliation must not cancel an active search"
        );
        service.cancel_search("main", "old");
        assert!(!new.load(Ordering::Acquire));
        service.cancel_search("main", "new");
        assert!(new.load(Ordering::Acquire));
        service.shutdown();
        assert!(quick.load(Ordering::Acquire));
    }
    #[test]
    fn live_edits_refresh_while_internal_caches_and_filename_only_scopes_stay_private() {
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("documents");
        let cache = folder.join("app-cache");
        let private = root.path().join("filename-only");
        fs::create_dir_all(&cache).unwrap();
        fs::create_dir(&private).unwrap();
        let source = folder.join("source.md");
        fs::write(&source, "initial public source").unwrap();
        fs::write(cache.join("secret.md"), "privatecanary").unwrap();
        fs::write(private.join("secret.md"), "privatecanary").unwrap();
        let service = ContentService::open(
            &root.path().join("index"),
            Err("not installed".into()),
            |_| {},
        )
        .unwrap();
        service.exclude_paths(&[cache]);
        let scopes = vec![
            SearchScope {
                id: "public".into(),
                name: "public".into(),
                path: folder.to_string_lossy().into(),
                filename_enabled: true,
                content_enabled: true,
                external_allowed: false,
            },
            SearchScope {
                id: "private".into(),
                name: "private".into(),
                path: private.to_string_lossy().into(),
                filename_enabled: true,
                content_enabled: false,
                external_allowed: false,
            },
        ];
        service.configure(scopes).unwrap();
        wait_ready(&service);
        assert_eq!(service.status().unwrap().indexed, 1);
        assert!(service
            .search("privatecanary", "", None, &AtomicBool::new(false))
            .unwrap()
            .hits
            .is_empty());
        fs::write(&source, "liveupdated public source with more text").unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if !service
                .search("liveupdated", "", None, &AtomicBool::new(false))
                .unwrap()
                .hits
                .is_empty()
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "Watcher must index edits without manual refresh"
            );
            thread::sleep(Duration::from_millis(100));
        }
        service.shutdown();
    }
}
