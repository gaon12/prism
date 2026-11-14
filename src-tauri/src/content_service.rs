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
use serde::{Deserialize, Serialize};
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

#[path = "unlock_requests.rs"]
mod unlock_requests;
const DIMENSIONS: usize = 768;
pub(crate) struct SemanticQuery {
    vector: Vec<f32>,
    fingerprint: String,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeywordSearch {
    pub query: String,
    pub extension: String,
    pub scope_id: Option<String>,
    pub content_revision: u64,
    pub settings_revision: u64,
}
const EXTRACTOR_VERSION: &str = "office-tables-v2";
const OCR_VERSION: &str = "native-ocr-v1";
const OCR_ENGINES_VERSION: &str = "ocr-engines-v2";
fn extraction_version(path: &Path, preferences: &crate::ocr::Preferences) -> String {
    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    if preferences.enabled
        && (extension.eq_ignore_ascii_case("pdf")
            || document_extract::is_image_extension(extension))
    {
        let engine = match preferences.engine {
            crate::ocr::Engine::Auto => "auto",
            crate::ocr::Engine::System => "system",
            crate::ocr::Engine::PaddleVl16 => "paddle-vl16",
        };
        let model = if preferences.engine == crate::ocr::Engine::System {
            "system".into()
        } else {
            crate::ocr_runtime::global()
                .map(|runtime| runtime.cache_tag())
                .unwrap_or_else(|| "not-loaded".into())
        };
        format!(
            "{EXTRACTOR_VERSION}-{OCR_ENGINES_VERSION}-{engine}-{}-{model}",
            preferences.language
        )
    } else {
        EXTRACTOR_VERSION.into()
    }
}
fn reusable_extraction(
    stored: Option<&str>,
    wanted: &str,
    preferences: &crate::ocr::Preferences,
) -> bool {
    stored == Some(wanted)
        || (!preferences.enabled
            && stored.is_some_and(|v| {
                v.starts_with(&format!("{EXTRACTOR_VERSION}-{OCR_VERSION}-"))
                    || v.starts_with(&format!("{EXTRACTOR_VERSION}-{OCR_ENGINES_VERSION}-"))
            }))
}
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
    /// Why phase is "paused" when the user did not pause it: "battery" while waiting for
    /// AC power under the pause-on-battery setting. None otherwise.
    pub pause_reason: Option<String>,
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
            pause_reason: None,
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
type PowerReader = Arc<dyn Fn() -> Option<bool> + Send + Sync>;
#[cfg(test)]
type ExtractObserver = Arc<dyn Fn(&Path) + Send + Sync>;
const POWER_POLL: Duration = Duration::from_secs(5);
struct Request {
    generation: u64,
    scopes: Vec<SearchScope>,
    cancel: Arc<AtomicBool>,
    ocr: crate::ocr::Preferences,
    file_policy: crate::file_policy::Categories,
}
struct ContentPolicy {
    revision: u64,
    categories: crate::file_policy::Categories,
    roots: Vec<PathBuf>,
}
fn admits_content(
    categories: crate::file_policy::Categories,
    roots: &[PathBuf],
    path: &Path,
    is_dir: bool,
) -> std::io::Result<bool> {
    if categories.unrestricted() {
        return Ok(true);
    }
    let canonical = crate::path_identity::canonical(path)?;
    let canonical_key = path_key(&canonical);
    let root = roots
        .iter()
        .filter(|root| under(&canonical_key, &path_key(root)))
        .max_by_key(|root| root.components().count())
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "No content policy root",
            )
        })?;
    crate::file_policy_fs::Reader::new(categories, root)
        .check(&canonical, is_dir)
        .map(|decision| decision == crate::file_policy::Decision::Include)
}
impl ContentPolicy {
    fn admits(&self, path: &Path) -> bool {
        admits_content(self.categories, &self.roots, path, false).unwrap_or(false)
    }
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
    ocr: Mutex<crate::ocr::Preferences>,
    file_policy: Mutex<crate::file_policy::Categories>,
    policy_revision: AtomicU64,
    #[cfg(test)]
    before_extract: Mutex<Option<ExtractObserver>>,
    status: Mutex<ContentStatus>,
    pending: Mutex<Option<Request>>,
    scan_cancel: Mutex<Arc<AtomicBool>>,
    searches: Mutex<HashMap<String, SearchTask>>,
    unlocks: unlock_requests::UnlockRequests,
    generation: AtomicU64,
    closed: AtomicBool,
    paused: AtomicBool,
    /// The user's "pause content indexing on battery" setting.
    battery_pause: AtomicBool,
    power_reader: PowerReader,
    power_poll: Duration,
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
        Self::open_with_power_reader(
            directory,
            embedding,
            listener,
            Arc::new(crate::power::on_battery),
            POWER_POLL,
        )
    }
    fn open_with_power_reader(
        directory: &Path,
        embedding: std::result::Result<EmbeddingRuntime, String>,
        listener: impl Fn(ContentStatus) + Send + Sync + 'static,
        power_reader: PowerReader,
        power_poll: Duration,
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
                ocr: Mutex::new(crate::ocr::Preferences::default()),
                file_policy: Mutex::new(crate::file_policy::Categories::default()),
                policy_revision: AtomicU64::new(0),
                #[cfg(test)]
                before_extract: Mutex::new(None),
                status: Mutex::new(ContentStatus::default()),
                pending: Mutex::new(None),
                scan_cancel: Mutex::new(Arc::new(AtomicBool::new(false))),
                searches: Mutex::new(HashMap::new()),
                unlocks: unlock_requests::UnlockRequests::default(),
                generation: AtomicU64::new(0),
                closed: AtomicBool::new(false),
                paused: AtomicBool::new(false),
                battery_pause: AtomicBool::new(false),
                power_reader,
                power_poll,
                listener: Arc::new(listener),
                last_emit: Mutex::new(Instant::now() - Duration::from_secs(1)),
                excluded: Mutex::new(vec![path_key(
                    &crate::path_identity::comparison(directory)
                        .unwrap_or_else(|_| directory.to_path_buf()),
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
            for path in paths {
                if let Ok(absolute) = std::path::absolute(path) {
                    excluded.push(path_key(&absolute));
                }
                if let Ok(canonical) = crate::path_identity::comparison(path) {
                    excluded.push(path_key(&canonical));
                }
            }
            excluded.sort();
            excluded.dedup();
        }
    }
    fn is_excluded(&self, path: &str) -> bool {
        let Ok(canonical) = crate::path_identity::comparison(Path::new(path)) else {
            return true;
        };
        let key = path_key(&canonical);
        self.inner
            .excluded
            .lock()
            .map(|roots| roots.iter().any(|root| under(&key, root)))
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
        self.configure_inner(scopes, None, None)
    }
    pub fn configure_with_ocr(
        &self,
        scopes: Vec<SearchScope>,
        ocr: crate::ocr::Preferences,
    ) -> Result<()> {
        ocr.validate()
            .map_err(|_| error("input", "Invalid OCR settings."))?;
        self.configure_inner(scopes, Some(ocr), None)
    }
    pub(crate) fn configure_with_policy(
        &self,
        scopes: Vec<SearchScope>,
        ocr: crate::ocr::Preferences,
        file_policy: crate::file_policy::Categories,
    ) -> Result<()> {
        ocr.validate()
            .map_err(|_| error("input", "Invalid OCR settings."))?;
        self.configure_inner(scopes, Some(ocr), Some(file_policy))
    }
    fn configure_inner(
        &self,
        scopes: Vec<SearchScope>,
        requested_ocr: Option<crate::ocr::Preferences>,
        requested_policy: Option<crate::file_policy::Categories>,
    ) -> Result<()> {
        self.inner.paused.store(false, Ordering::Release);
        if let Ok(cancel) = self.inner.scan_cancel.lock() {
            cancel.store(true, Ordering::Release);
        }
        let mut current_scopes = self
            .inner
            .scopes
            .lock()
            .map_err(|_| error("worker", "Content scopes unavailable."))?;
        let mut current_ocr = self
            .inner
            .ocr
            .lock()
            .map_err(|_| error("worker", "OCR settings unavailable."))?;
        let ocr = requested_ocr.unwrap_or_else(|| current_ocr.clone());
        let mut current_policy = self
            .inner
            .file_policy
            .lock()
            .map_err(|_| error("worker", "Content policy unavailable."))?;
        let file_policy = requested_policy.unwrap_or(*current_policy);
        let changed = *current_ocr != ocr
            || *current_policy != file_policy
            || current_scopes.len() != scopes.len()
            || current_scopes.iter().zip(&scopes).any(|(left, right)| {
                left.id != right.id
                    || left.path != right.path
                    || left.content_enabled != right.content_enabled
            });
        *current_scopes = scopes.clone();
        *current_ocr = ocr.clone();
        *current_policy = file_policy;
        self.queue(scopes, ocr, file_policy, changed)?;
        drop(current_policy);
        drop(current_ocr);
        drop(current_scopes);
        self.update(|status| {
            status.phase = "indexing".into();
            status.current_path = None;
        });
        Ok(())
    }
    // Called while the configuration locks are held, so concurrent refresh and
    // settings changes cannot enqueue an older policy after a newer one.
    fn queue(
        &self,
        scopes: Vec<SearchScope>,
        ocr: crate::ocr::Preferences,
        file_policy: crate::file_policy::Categories,
        changed: bool,
    ) -> Result<()> {
        let generation = self.inner.generation.fetch_add(1, Ordering::SeqCst) + 1;
        if changed {
            self.inner.policy_revision.fetch_add(1, Ordering::SeqCst);
        }
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
            ocr,
            file_policy,
        });
        if changed {
            self.inner.unlocks.cancel_all(false);
        }
        if let Ok(searches) = self.inner.searches.lock() {
            if changed {
                for task in searches.values() {
                    task.cancel.store(true, Ordering::Release);
                }
            }
        }
        Ok(())
    }
    pub fn refresh(&self) -> Result<()> {
        self.inner.paused.store(false, Ordering::Release);
        let scopes = self
            .inner
            .scopes
            .lock()
            .map_err(|_| error("worker", "Content scopes unavailable."))?;
        let ocr = self
            .inner
            .ocr
            .lock()
            .map_err(|_| error("worker", "OCR settings unavailable."))?;
        let file_policy = self
            .inner
            .file_policy
            .lock()
            .map_err(|_| error("worker", "Content policy unavailable."))?;
        self.queue(scopes.clone(), ocr.clone(), *file_policy, false)?;
        drop(file_policy);
        drop(ocr);
        drop(scopes);
        self.update(|status| {
            status.phase = "indexing".into();
            status.current_path = None;
        });
        Ok(())
    }
    /// Applies the "pause content indexing on battery" setting. Turning it off lets any
    /// waiting work start on the worker's next turn.
    pub fn set_battery_pause(&self, enabled: bool) {
        self.inner.battery_pause.store(enabled, Ordering::Release);
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
            status.pause_reason = None;
        });
    }
    pub fn shutdown(&self) {
        self.inner.closed.store(true, Ordering::Release);
        self.inner.unlocks.cancel_all(true);
        self.cancel_index();
        if let Ok(searches) = self.inner.searches.lock() {
            for search in searches.values() {
                search.cancel.store(true, Ordering::Release);
            }
        }
    }
    fn monitor(&self) {
        let mut last_scan = Instant::now();
        // Idle scans share this cache. A running scan has its own bounded observer
        // because this scheduler waits synchronously for extraction to finish.
        let mut power_checked: Option<Instant> = None;
        let mut on_battery = false;
        let mut held_for_battery = false;
        let mut watcher: Option<RecommendedWatcher> = None;
        let mut watched_roots = Vec::new();
        let mut last_watch_attempt = Instant::now() - Duration::from_secs(60);
        loop {
            if self.inner.closed.load(Ordering::Acquire) || Arc::strong_count(&self.inner) == 1 {
                return;
            }
            // Unknown power counts as plugged in. Explicit manual pause is independent
            // of battery pause and must survive reconnecting the charger.
            if self.inner.battery_pause.load(Ordering::Acquire) {
                if power_checked.is_none_or(|at| at.elapsed() >= self.inner.power_poll) {
                    on_battery = (self.inner.power_reader)() == Some(true);
                    power_checked = Some(Instant::now());
                }
            } else {
                on_battery = false;
                power_checked = None;
            }
            let work_waiting = self
                .inner
                .pending
                .lock()
                .is_ok_and(|pending| pending.is_some())
                || (self.inner.generation.load(Ordering::Acquire) > 0
                    && !self.inner.paused.load(Ordering::Acquire)
                    && (self.inner.dirty.load(Ordering::Acquire)
                        || last_scan.elapsed() >= Duration::from_secs(60)));
            if on_battery && work_waiting {
                let needs_notice = !held_for_battery
                    || self.status().is_ok_and(|status| {
                        status.phase != "paused"
                            || status.pause_reason.as_deref() != Some("battery")
                    });
                if needs_notice {
                    held_for_battery = true;
                    self.update(|status| {
                        if !self.inner.paused.load(Ordering::Acquire) {
                            status.phase = "paused".into();
                            status.current_path = None;
                            status.pause_reason = Some("battery".into());
                        }
                    });
                }
                thread::sleep(Duration::from_millis(250));
                continue;
            }
            if held_for_battery {
                held_for_battery = false;
                // Back on AC power (or the setting was turned off): rescan right away.
                self.update(|status| {
                    status.pause_reason = None;
                    if status.phase == "paused" {
                        status.phase =
                            if work_waiting && !self.inner.paused.load(Ordering::Acquire) {
                                "indexing"
                            } else {
                                "ready"
                            }
                            .into();
                    }
                });
                if !self.inner.paused.load(Ordering::Acquire) {
                    self.inner.dirty.store(true, Ordering::Release);
                    last_scan = Instant::now() - Duration::from_secs(60);
                }
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
                // The scan's observer may have noticed a power transition while
                // this scheduler was busy. Never reuse the pre-scan AC sample.
                power_checked = None;
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
            .filter_map(|scope| crate::path_identity::canonical(Path::new(&scope.path)).ok())
            .filter(|path| path.is_dir())
            .collect();
        roots.sort();
        roots.dedup();
        roots
    }
    fn query_policy(&self, ids: &[String]) -> Result<(ContentPolicy, Vec<PathBuf>)> {
        // Same lock order as configure/refresh. Explicit nested roots also
        // define attribute ancestry when the query selected their parent.
        let scopes = self
            .inner
            .scopes
            .lock()
            .map_err(|_| error("worker", "Content scopes unavailable."))?;
        let categories = *self
            .inner
            .file_policy
            .lock()
            .map_err(|_| error("worker", "Content policy unavailable."))?;
        let selected = if ids.is_empty() {
            Self::roots(&scopes, None)
        } else {
            Self::roots(
                &scopes
                    .iter()
                    .filter(|scope| ids.contains(&scope.id))
                    .cloned()
                    .collect::<Vec<_>>(),
                None,
            )
        };
        let roots = if categories.unrestricted() {
            Vec::new()
        } else if ids.is_empty() {
            selected.clone()
        } else {
            Self::roots(&scopes, None)
        };
        Ok((
            ContentPolicy {
                revision: self.inner.policy_revision.load(Ordering::Acquire),
                categories,
                roots,
            },
            selected,
        ))
    }
    fn check_query_policy(&self, policy: &ContentPolicy) -> Result<()> {
        if policy.revision != self.inner.policy_revision.load(Ordering::Acquire)
            || self.inner.closed.load(Ordering::Acquire)
        {
            Err(error(
                "sourceChanged",
                "Content permissions changed. Search again.",
            ))
        } else {
            Ok(())
        }
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
        // A scoped observer lives exactly as long as this scan. It only signals
        // this request's existing cancellation token, which extraction/OCR, SQL
        // and embedding already honor. It never cancels queries or changes the
        // user's manual pause, scopes, permissions or running model process.
        std::thread::scope(|scope| {
            let request = &request;
            let (finished, receiver) = std::sync::mpsc::channel();
            scope.spawn(move || loop {
                match receiver.recv_timeout(self.inner.power_poll) {
                    Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                }
                if !self.current(request) {
                    break;
                }
                if self.inner.battery_pause.load(Ordering::Acquire)
                    && (self.inner.power_reader)() == Some(true)
                    && self.inner.battery_pause.load(Ordering::Acquire)
                    && self.current(request)
                {
                    request.cancel.store(true, Ordering::Release);
                    // Retain completed documents and retry the interrupted scan
                    // with a fresh token once the scheduler observes AC again.
                    self.inner.dirty.store(true, Ordering::Release);
                    break;
                }
            });
            self.scan_inner(request);
            let _ = finished.send(());
        });
    }
    fn scan_inner(&self, request: &Request) {
        if !self.current(request) {
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
            } else if crate::path_identity::canonical(Path::new(&scope.path)).is_err() {
                self.issue(
                    Path::new(&scope.path),
                    "unsafePath",
                    "Content folder contains a link or an inaccessible ancestor.",
                );
            }
        }
        if let Err(error) = self
            .inner
            .store
            .purge_for_scopes(&roots, &unavailable, &request.cancel)
        {
            if self.current(request) {
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
            if !self.current(request) {
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
                    if !self.current(request) {
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
                if self.current(request) {
                    self.update(|status| status.embedding_error = Some(error.message));
                }
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
                    self.current(request)
                        && !is_link(entry.path())
                        && !excluded
                            .iter()
                            .any(|root| under(&path_key(entry.path()), root))
                        && (!entry.file_type().is_dir()
                            || match admits_content(request.file_policy, &roots, entry.path(), true)
                            {
                                Ok(allowed) => allowed,
                                Err(_) => {
                                    self.issue(
                                        entry.path(),
                                        "policy",
                                        "Content policy evidence is unavailable.",
                                    );
                                    false
                                }
                            })
                })
            {
                if !self.current(request) {
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
                match admits_content(request.file_policy, &roots, path, false) {
                    Ok(true) => {}
                    Ok(false) => continue,
                    Err(_) => {
                        self.issue(path, "policy", "Content policy evidence is unavailable.");
                        continue;
                    }
                }
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
                        reusable_extraction(
                            document.extractor_version.as_deref(),
                            &extraction_version(path, &request.ocr),
                            &request.ocr,
                        )
                    });
                // Existing OCR text remains available when OCR is switched off.
                // New image payloads are neither read nor added to the issue log.
                if previous.is_none()
                    && !request.ocr.enabled
                    && document_extract::is_image_extension(
                        path.extension().and_then(|e| e.to_str()).unwrap_or(""),
                    )
                {
                    continue;
                }
                let version = extraction_version(path, &request.ocr);
                let document = if let Some(previous) = previous {
                    previous
                } else {
                    #[cfg(test)]
                    if let Some(observer) = self.inner.before_extract.lock().unwrap().clone() {
                        observer(path);
                    }
                    if !self.current(request) {
                        return;
                    }
                    let extracted = match document_extract::extract_document(
                        path,
                        &ExtractOptions {
                            ocr: request.ocr.clone(),
                            ..Default::default()
                        },
                        &request.cancel,
                    ) {
                        Ok(extracted) => extracted,
                        Err(error) => {
                            if !self.current(request) {
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
                    if !self.current(request) {
                        return;
                    }
                    // Configuration cannot change between this final authority
                    // check and committing the extracted text.
                    let configuration = match self.inner.scopes.lock() {
                        Ok(configuration) => configuration,
                        Err(_) => return,
                    };
                    if !self.current(request) {
                        return;
                    }
                    if !matches!(
                        admits_content(request.file_policy, &roots, path, false),
                        Ok(true)
                    ) {
                        continue;
                    }
                    let publication = self.inner.store.replace_document_versioned(
                        &extracted,
                        &roots,
                        Some(&version),
                        &request.cancel,
                    );
                    drop(configuration);
                    match publication {
                        Ok(document) => document,
                        Err(error) => {
                            if !self.current(request) {
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
                if let Some(vector) = &vector {
                    if let Err(error) =
                        self.embed_document(&document, vector, &roots, &request.cancel)
                    {
                        if self.current(request) {
                            self.update(|status| status.embedding_error = Some(error.message));
                        }
                    } else {
                        self.update(|status| status.embedded += 1);
                    }
                }
                self.update(|status| status.indexed += 1);
            }
        }
        if !self.current(request) {
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
    pub(crate) fn snapshot_keyword_files<E>(
        &self,
        search: &KeywordSearch,
        cancel: &AtomicBool,
        visit: &mut dyn FnMut(&str) -> std::result::Result<(), E>,
    ) -> std::result::Result<bool, crate::content_store::SnapshotError<E, ContentError>> {
        use crate::content_store::SnapshotError::{Source, Visitor};
        let before = self.status().map_err(Source)?;
        let generation = self.inner.generation.load(Ordering::Acquire);
        if before.revision != search.content_revision {
            return Err(Source(error(
                "sourceChanged",
                "The content index changed. Search again.",
            )));
        }
        let filter = self.search_filter(&search.extension).map_err(Source)?;
        let scopes: Vec<_> = search.scope_id.iter().cloned().collect();
        let (policy, roots) = self.query_policy(&scopes).map_err(Source)?;
        self.inner
            .store
            .snapshot_keyword_files(
                &search.query,
                &filter.extensions,
                &roots,
                cancel,
                &mut |path| {
                    if self.is_excluded(path) || !policy.admits(Path::new(path)) {
                        Ok(())
                    } else {
                        visit(path)
                    }
                },
            )
            .map_err(|failure| match failure {
                Visitor(value) => Visitor(value),
                Source(value) => Source(error(
                    if cancel.load(Ordering::Acquire) {
                        "cancelled"
                    } else {
                        "source"
                    },
                    &value.message,
                )),
            })?;
        let after = self.status().map_err(Source)?;
        self.check_query_policy(&policy).map_err(Source)?;
        if cancel.load(Ordering::Acquire) {
            return Err(Source(error("cancelled", "Search cancelled.")));
        }
        if before.revision != after.revision
            || generation != self.inner.generation.load(Ordering::Acquire)
        {
            return Err(Source(error(
                "sourceChanged",
                "The content index changed. Search again.",
            )));
        }
        Ok(before.phase != "ready" || before.skipped > 0 || !before.issues.is_empty())
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
        self.search_selected_with_admission(
            query,
            extension,
            scope_ids,
            documents,
            cancel,
            &mut |_| true,
        )
    }
    pub(crate) fn search_selected_with_admission(
        &self,
        query: &str,
        extension: &str,
        scope_ids: &[String],
        documents: Option<&[String]>,
        cancel: &AtomicBool,
        admit: &mut dyn FnMut(&Path) -> bool,
    ) -> Result<ContentSearchResponse> {
        let (policy, roots) = self.query_policy(scope_ids)?;
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
            .search_detailed_with_admission(query, &filter, &roots, 100, cancel, &mut |path| {
                !self.is_excluded(&path.to_string_lossy()) && policy.admits(path) && admit(path)
            })
            .map_err(|error| ContentError {
                code: serde_json::to_value(error.code)
                    .ok()
                    .and_then(|value| value.as_str().map(String::from))
                    .unwrap_or_else(|| "source".into()),
                message: error.message,
            })?;
        response.hits.retain(|hit| {
            !self.is_excluded(&hit.chunk.file_path)
                && policy.admits(Path::new(&hit.chunk.file_path))
        });
        self.check_query_policy(&policy)?;
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
        self.run_semantic_query_with_admission(query, filter, scope_ids, limit, cancel, &mut |_| {
            true
        })
    }
    pub(crate) fn run_semantic_query_with_admission(
        &self,
        query: &SemanticQuery,
        filter: &ContentSearchFilter,
        scope_ids: &[String],
        limit: usize,
        cancel: &AtomicBool,
        admit: &mut (dyn FnMut(&Path) -> bool + Send),
    ) -> Result<VectorSearchResponse> {
        if !(1..=128).contains(&limit)
            || !filter.document_paths.is_empty() && filter.document_paths.len() > 256
        {
            return Err(error("input", "Invalid semantic candidate budget."));
        }
        let (policy, roots) = self.query_policy(scope_ids)?;
        let vector = self.vector()?;
        let mut response = self
            .inner
            .runtime
            .block_on(vector.search_with_admission(
                &query.vector,
                &query.fingerprint,
                filter,
                &self.inner.store,
                &roots,
                limit,
                4096,
                cancel,
                &mut |path| {
                    !self.is_excluded(&path.to_string_lossy()) && policy.admits(path) && admit(path)
                },
            ))
            .map_err(|error| ContentError {
                code: "vector".into(),
                message: error.message,
            })?;
        let before = response.hits.len();
        response.hits.retain(|hit| {
            !self.is_excluded(&hit.chunk.file_path)
                && policy.admits(Path::new(&hit.chunk.file_path))
        });
        response.filtered_out += before - response.hits.len();
        self.check_query_policy(&policy)?;
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
        let (policy, roots) = self.query_policy(&ids)?;
        let current = self
            .inner
            .store
            .chunks_by_ids(&requested_ids, &roots, cancel)
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
                && policy.admits(Path::new(&hit.chunk.file_path))
                && current.get(&hit.chunk.id).is_some_and(|chunk| {
                    chunk.file_path == hit.chunk.file_path
                        && chunk.fingerprint == hit.chunk.fingerprint
                })
        });
        if cancel.load(Ordering::Acquire) {
            return Err(error("cancelled", "Search cancelled."));
        }
        self.check_query_policy(&policy)?;
        Ok(result)
    }
    pub fn chunks(&self, ids: &[String], cancel: &AtomicBool) -> Result<Vec<ContentChunk>> {
        let (policy, roots) = self.query_policy(&[])?;
        let mut chunks = self
            .inner
            .store
            .chunks_by_ids(ids, &roots, cancel)
            .map_err(|error| ContentError {
                code: "source".into(),
                message: error.message,
            })?;
        chunks.retain(|chunk| {
            !self.is_excluded(&chunk.file_path) && policy.admits(Path::new(&chunk.file_path))
        });
        self.check_query_policy(&policy)?;
        Ok(chunks)
    }
    pub(crate) fn prepare_unlock(&self, window: &str) -> Result<String> {
        self.inner
            .unlocks
            .prepare(window)
            .map_err(|value| match value {
                unlock_requests::StartError::Cancelled => {
                    error("cancelled", "document retry cancelled.")
                }
                unlock_requests::StartError::Busy => {
                    error("busy", "A previous document retry is still finishing.")
                }
                unlock_requests::StartError::Unavailable => {
                    error("worker", "Could not prepare the document retry.")
                }
            })
    }
    pub(crate) fn begin_unlock(
        &self,
        window: &str,
        id: &str,
    ) -> Result<unlock_requests::UnlockPermit> {
        if id.is_empty() || id.len() > 80 {
            return Err(error("input", "Invalid document retry identifier."));
        }
        self.inner
            .unlocks
            .start(window, id)
            .map_err(|value| match value {
                unlock_requests::StartError::Cancelled => {
                    error("cancelled", "document retry cancelled.")
                }
                unlock_requests::StartError::Unavailable => {
                    error("worker", "Could not prepare the document retry.")
                }
                unlock_requests::StartError::Busy => {
                    error("busy", "A previous document retry is still finishing.")
                }
            })
    }
    pub(crate) fn cancel_unlock(&self, window: &str, id: &str) {
        if !id.is_empty() && id.len() <= 80 {
            self.inner.unlocks.cancel(window, id);
        }
    }
    fn unlock_path(&self, path: &Path, scopes: &[SearchScope]) -> Result<(PathBuf, Vec<PathBuf>)> {
        let roots = Self::roots(scopes, None);
        let canonical = crate::path_identity::canonical(path)
            .map_err(|_| error("unsafePath", "The document path is missing or unsafe."))?;
        if !canonical.is_file()
            || !roots.iter().any(|root| canonical.starts_with(root))
            || self.is_excluded(&canonical.to_string_lossy())
            || !admits_content(
                *self
                    .inner
                    .file_policy
                    .lock()
                    .map_err(|_| error("worker", "Content policy unavailable."))?,
                &roots,
                &canonical,
                false,
            )
            .unwrap_or(false)
        {
            return Err(error(
                "outsideScope",
                "The document is outside current content folders.",
            ));
        }
        if !canonical
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| {
                matches!(
                    value.to_ascii_lowercase().as_str(),
                    "pdf" | "docx" | "xlsx" | "pptx"
                )
            })
        {
            return Err(error(
                "unsupportedFormat",
                "Password retry supports PDF and OOXML documents.",
            ));
        }
        Ok((canonical, roots))
    }
    pub(crate) fn unlock_pdf(
        &self,
        path: &Path,
        password: &str,
        cancel: &AtomicBool,
    ) -> Result<ContentDocumentSummary> {
        self.unlock_document(path, password, cancel, &["pdf"])
    }
    pub(crate) fn unlock_office(
        &self,
        path: &Path,
        password: &str,
        cancel: &AtomicBool,
    ) -> Result<ContentDocumentSummary> {
        self.unlock_document(path, password, cancel, &["docx", "xlsx", "pptx"])
    }
    fn unlock_document(
        &self,
        path: &Path,
        password: &str,
        cancel: &AtomicBool,
        formats: &[&str],
    ) -> Result<ContentDocumentSummary> {
        if self.inner.closed.load(Ordering::Acquire) || cancel.load(Ordering::Acquire) {
            return Err(error("cancelled", "Document retry cancelled."));
        }
        if password.len() > 1024 {
            return Err(error(
                "input",
                "Document password exceeds the input budget.",
            ));
        }
        let (canonical, ocr, policy_revision) = {
            let scopes = self
                .inner
                .scopes
                .lock()
                .map_err(|_| error("worker", "Content scopes unavailable."))?;
            let canonical = self.unlock_path(path, &scopes)?.0;
            let ocr = self
                .inner
                .ocr
                .lock()
                .map_err(|_| error("worker", "OCR settings unavailable."))?
                .clone();
            (
                canonical,
                ocr,
                self.inner.policy_revision.load(Ordering::Acquire),
            )
        };
        if !canonical
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| formats.contains(&value.to_ascii_lowercase().as_str()))
        {
            return Err(error(
                "unsupportedFormat",
                "This password command does not support that document format.",
            ));
        }
        let extracted = document_extract::extract_document_with_password(
            &canonical,
            &ExtractOptions {
                ocr: ocr.clone(),
                ..Default::default()
            },
            cancel,
            password,
        )
        .map_err(|value| {
            error(
                serde_json::to_value(value.code)
                    .ok()
                    .and_then(|value| value.as_str().map(String::from))
                    .as_deref()
                    .unwrap_or("extract"),
                &value.message,
            )
        })?;
        self.publish_unlocked(path, &extracted, &ocr, cancel, policy_revision)
    }
    fn publish_unlocked(
        &self,
        path: &Path,
        extracted: &document_extract::ExtractedDocument,
        ocr: &crate::ocr::Preferences,
        cancel: &AtomicBool,
        policy_revision: u64,
    ) -> Result<ContentDocumentSummary> {
        use sha2::{Digest, Sha256};
        use std::io::Read;
        // Serialize the final authorization and publication against configure's
        // scope update. No password is kept in the service, status or repository.
        let scopes = self
            .inner
            .scopes
            .lock()
            .map_err(|_| error("worker", "Content scopes unavailable."))?;
        let (canonical, roots) = self.unlock_path(path, &scopes)?;
        if policy_revision != self.inner.policy_revision.load(Ordering::Acquire) {
            return Err(error(
                "cancelled",
                "Content permissions changed during the document retry.",
            ));
        }
        let current_ocr = self
            .inner
            .ocr
            .lock()
            .map_err(|_| error("worker", "OCR settings unavailable."))?;
        if *current_ocr != *ocr {
            return Err(error(
                "cancelled",
                "OCR settings changed during the document retry.",
            ));
        }
        if path_key(&canonical) != path_key(Path::new(&extracted.file_path)) {
            return Err(error(
                "sourceChanged",
                "The document changed during the retry.",
            ));
        }
        let mut file = fs::File::open(&canonical)
            .map_err(|_| error("sourceChanged", "The document is unavailable."))?;
        let mut digest = Sha256::new();
        let mut buffer = [0u8; 32768];
        let mut size = 0u64;
        loop {
            if cancel.load(Ordering::Acquire) || self.inner.closed.load(Ordering::Acquire) {
                return Err(error("cancelled", "document retry cancelled."));
            }
            let count = file
                .read(&mut buffer)
                .map_err(|_| error("sourceChanged", "The document changed during the retry."))?;
            if count == 0 {
                break;
            }
            size += count as u64;
            if size > extracted.fingerprint.size_bytes {
                return Err(error(
                    "sourceChanged",
                    "The document changed during the retry.",
                ));
            }
            digest.update(&buffer[..count]);
        }
        if size != extracted.fingerprint.size_bytes
            || format!("{:x}", digest.finalize()) != extracted.fingerprint.sha256
        {
            return Err(error(
                "sourceChanged",
                "The document changed during the retry.",
            ));
        }
        let already_indexed = self
            .inner
            .store
            .document_summary(&canonical, &roots, cancel)
            .map_err(|_| error("storage", "Could not inspect the content index."))?
            .is_some();
        let document = self
            .inner
            .store
            .replace_document_versioned(
                extracted,
                &roots,
                Some(&extraction_version(&canonical, ocr)),
                cancel,
            )
            .map_err(|value| {
                error(
                    if cancel.load(Ordering::Acquire) {
                        "cancelled"
                    } else {
                        match value.code {
                            crate::content_store::ContentStoreErrorCode::SourceChanged => {
                                "sourceChanged"
                            }
                            crate::content_store::ContentStoreErrorCode::OutsideScope => {
                                "outsideScope"
                            }
                            crate::content_store::ContentStoreErrorCode::UnsafePath => "unsafePath",
                            _ => "storage",
                        }
                    },
                    &value.message,
                )
            })?;
        drop(current_ocr);
        drop(scopes);
        self.update(|status| {
            let before = status.issues.len();
            status
                .issues
                .retain(|issue| path_key(Path::new(&issue.path)) != path_key(&canonical));
            status.skipped = status.skipped.saturating_sub(before - status.issues.len());
            if !already_indexed {
                status.indexed += 1;
            }
            status.discovered = status.discovered.max(status.indexed + status.skipped);
        });
        // Preserve an explicitly paused background index. Otherwise schedule the
        // normal model worker; importing a password never starts a model itself.
        if !self.inner.paused.load(Ordering::Acquire) {
            let _ = self.refresh();
        }
        Ok(document)
    }
    pub fn resolve_document(&self, path: &Path) -> Result<PathBuf> {
        let (policy, roots) = self.query_policy(&[])?;
        if self.is_excluded(&path.to_string_lossy()) || !policy.admits(path) {
            return Err(error(
                "source",
                "App cache files cannot be used as document sources.",
            ));
        }
        let resolved = self
            .inner
            .store
            .document_summary(path, &roots, &AtomicBool::new(false))
            .map_err(|_| error("source", "Content source is unavailable."))?
            .map(|document| PathBuf::from(document.file_path))
            .ok_or_else(|| {
                error(
                    "source",
                    "This document is no longer a current authorized source.",
                )
            })?;
        self.check_query_policy(&policy)?;
        Ok(resolved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait_until(label: &str, condition: impl Fn() -> bool) {
        let start = Instant::now();
        while !condition() {
            assert!(start.elapsed() < Duration::from_secs(5), "{label}");
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn policy_scope(path: &Path, id: &str) -> SearchScope {
        SearchScope {
            id: id.into(),
            name: id.into(),
            path: path.to_string_lossy().into(),
            filename_enabled: true,
            content_enabled: true,
            external_allowed: false,
        }
    }

    #[test]
    fn inclusion_policy_gates_extraction_and_cached_queries_and_survives_refresh() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("sources");
        fs::create_dir_all(root.join(".private")).unwrap();
        fs::create_dir_all(root.join("generated")).unwrap();
        fs::create_dir(root.join(".git")).unwrap();
        fs::write(root.join(".gitignore"), "/generated/\n").unwrap();
        for name in [
            "notes.txt",
            "main.rs",
            ".private/hidden.txt",
            "generated/ignored.txt",
        ] {
            fs::write(root.join(name), "contentpolicycanary evidence").unwrap();
        }
        fs::write(root.join(".private/locked.pdf"), b"not a PDF").unwrap();
        let service = ContentService::open(
            &directory.path().join("index"),
            Err("not installed".into()),
            |_| {},
        )
        .unwrap();
        let observed = Arc::new(Mutex::new(Vec::<PathBuf>::new()));
        let recorded = observed.clone();
        *service.inner.before_extract.lock().unwrap() = Some(Arc::new(move |path| {
            recorded.lock().unwrap().push(path.to_path_buf());
        }));
        let scopes = vec![policy_scope(&root, "docs")];
        let restricted = crate::file_policy::Categories {
            hidden: false,
            source_code: false,
            repository_ignored: false,
            ..Default::default()
        };
        service
            .configure_with_policy(scopes.clone(), Default::default(), restricted)
            .unwrap();
        wait_ready(&service);
        let calls = observed.lock().unwrap().clone();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].ends_with("notes.txt"));
        let cancel = AtomicBool::new(false);
        assert_eq!(
            service
                .unlock_pdf(&root.join(".private/locked.pdf"), "unused", &cancel)
                .unwrap_err()
                .code,
            "outsideScope"
        );
        let first = service
            .search("contentpolicycanary", "", None, &cancel)
            .unwrap();
        assert_eq!(first.hits.len(), 1);
        for name in ["main.rs", ".private/hidden.txt", "generated/ignored.txt"] {
            assert!(service
                .inner
                .store
                .document_summary(&root.join(name), std::slice::from_ref(&root), &cancel)
                .unwrap()
                .is_none());
        }
        service
            .configure_with_policy(scopes.clone(), Default::default(), Default::default())
            .unwrap();
        wait_ready(&service);
        let all = service
            .search("contentpolicycanary", "", None, &cancel)
            .unwrap();
        assert_eq!(all.hits.len(), 4);
        let denied: Vec<_> = all
            .hits
            .iter()
            .filter(|hit| !hit.chunk.file_path.ends_with("notes.txt"))
            .map(|hit| hit.chunk.id.clone())
            .collect();
        let old_search = service.begin_search("main", "old_policy").unwrap();
        service
            .configure_with_policy(scopes.clone(), Default::default(), restricted)
            .unwrap();
        assert!(old_search.load(Ordering::Acquire));
        wait_ready(&service);
        assert!(service.chunks(&denied, &cancel).unwrap().is_empty());
        assert!(service.resolve_document(&root.join("main.rs")).is_err());
        // The legacy OCR-only adapter must preserve an already configured policy.
        service
            .configure_with_ocr(scopes.clone(), Default::default())
            .unwrap();
        wait_ready(&service);
        service.refresh().unwrap();
        wait_ready(&service);
        assert_eq!(
            service
                .search("contentpolicycanary", "", None, &cancel)
                .unwrap()
                .hits
                .len(),
            1
        );
        let status = service.status().unwrap();
        let search = KeywordSearch {
            query: "contentpolicycanary".into(),
            extension: "".into(),
            scope_id: None,
            content_revision: status.revision,
            settings_revision: 1,
        };
        let mut selected = Vec::new();
        service
            .snapshot_keyword_files(&search, &cancel, &mut |path| {
                selected.push(path.to_owned());
                Ok::<_, ()>(())
            })
            .unwrap();
        assert_eq!(selected.len(), 1);
        assert!(selected[0].ends_with("notes.txt"));
        service
            .configure_with_policy(scopes.clone(), Default::default(), Default::default())
            .unwrap();
        wait_ready(&service);
        let mut changed = false;
        let error = service
            .search_selected_with_admission(
                "contentpolicycanary",
                "",
                &[],
                None,
                &cancel,
                &mut |_| {
                    if !changed {
                        changed = true;
                        service
                            .configure_with_policy(scopes.clone(), Default::default(), restricted)
                            .unwrap();
                    }
                    true
                },
            )
            .unwrap_err();
        assert_eq!(error.code, "sourceChanged");
        wait_ready(&service);
        service.shutdown();
    }

    #[test]
    fn explicit_nested_content_root_resets_hidden_ancestry_but_filename_only_root_does_not() {
        let directory = tempfile::tempdir().unwrap();
        let parent = directory.path().join("sources");
        let nested = parent.join(".private/docs");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("allowed.txt"), "nestedpolicycanary evidence").unwrap();
        fs::write(
            parent.join(".private/denied.txt"),
            "nestedpolicycanary evidence",
        )
        .unwrap();
        let service = ContentService::open(
            &directory.path().join("index"),
            Err("not installed".into()),
            |_| {},
        )
        .unwrap();
        let mut scopes = vec![
            policy_scope(&parent, "parent"),
            policy_scope(&nested, "nested"),
        ];
        let policy = crate::file_policy::Categories {
            hidden: false,
            ..Default::default()
        };
        service
            .configure_with_policy(scopes.clone(), Default::default(), policy)
            .unwrap();
        wait_ready(&service);
        let cancel = AtomicBool::new(false);
        let hits = service
            .search("nestedpolicycanary", "", Some("parent"), &cancel)
            .unwrap();
        assert_eq!(hits.hits.len(), 1);
        assert!(hits.hits[0].chunk.file_path.ends_with("allowed.txt"));
        scopes[1].content_enabled = false;
        service
            .configure_with_policy(scopes, Default::default(), policy)
            .unwrap();
        wait_ready(&service);
        assert!(service
            .search("nestedpolicycanary", "", None, &cancel)
            .unwrap()
            .hits
            .is_empty());
        service.shutdown();
    }

    #[test]
    fn category_change_cancels_an_old_extraction_before_it_can_publish() {
        use std::sync::mpsc;
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("sources");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("old.rs"), "oldpolicypublicationcanary").unwrap();
        let service = ContentService::open(
            &directory.path().join("index"),
            Err("not installed".into()),
            |_| {},
        )
        .unwrap();
        let (started, receiving) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let wait = Mutex::new(wait);
        *service.inner.before_extract.lock().unwrap() = Some(Arc::new(move |_| {
            started.send(()).unwrap();
            wait.lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(3))
                .unwrap();
        }));
        let scopes = vec![policy_scope(&root, "docs")];
        service.configure(scopes.clone()).unwrap();
        receiving.recv_timeout(Duration::from_secs(3)).unwrap();
        service
            .configure_with_policy(
                scopes,
                Default::default(),
                crate::file_policy::Categories {
                    source_code: false,
                    ..Default::default()
                },
            )
            .unwrap();
        release.send(()).unwrap();
        wait_ready(&service);
        assert!(service
            .inner
            .store
            .document_summary(
                &root.join("old.rs"),
                std::slice::from_ref(&root),
                &AtomicBool::new(false)
            )
            .unwrap()
            .is_none());
        assert!(service
            .search(
                "oldpolicypublicationcanary",
                "",
                None,
                &AtomicBool::new(false)
            )
            .unwrap()
            .hits
            .is_empty());
        service.shutdown();
    }

    #[test]
    fn battery_interrupts_an_active_scan_preserves_search_and_resumes_with_a_fresh_token() {
        use std::sync::mpsc;
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("sources");
        fs::create_dir(&folder).unwrap();
        let cached = folder.join("cached.md");
        let busy = folder.join("busy.md");
        fs::write(&cached, "retainedbatterycanary cached original").unwrap();
        fs::write(&busy, "resumedbatterycanary interrupted original").unwrap();
        let battery = Arc::new(AtomicBool::new(false));
        let sample = battery.clone();
        let gated = Arc::new(AtomicBool::new(false));
        let gate = gated.clone();
        let (entered, observed) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let released = Mutex::new(released);
        let service = ContentService::open_with_power_reader(
            &root.path().join("cache"),
            Err("no model".into()),
            move |status| {
                if status.phase == "ready" && !gate.swap(true, Ordering::AcqRel) {
                    let _ = entered.send(());
                    // Hold the actual worker in its final notification, before
                    // its scan scope exits. Progress updates are throttled, so
                    // only a phase transition is a deterministic listener gate.
                    // The independent observer must cancel its active token
                    // while this worker cannot advance the scheduler itself.
                    let _ = released
                        .lock()
                        .unwrap()
                        .recv_timeout(Duration::from_secs(5));
                }
            },
            Arc::new(move || Some(sample.load(Ordering::Acquire))),
            Duration::from_millis(20),
        )
        .unwrap();
        let roots = vec![crate::path_identity::canonical(&folder).unwrap()];
        let cancel = AtomicBool::new(false);
        let extracted =
            document_extract::extract_document(&cached, &ExtractOptions::default(), &cancel)
                .unwrap();
        service
            .inner
            .store
            .replace_document_versioned(&extracted, &roots, Some(EXTRACTOR_VERSION), &cancel)
            .unwrap();
        let scope = SearchScope {
            id: "docs".into(),
            name: "docs".into(),
            path: folder.to_string_lossy().into(),
            filename_enabled: true,
            content_enabled: true,
            external_allowed: false,
        };
        service.set_battery_pause(true);
        service.configure(vec![scope]).unwrap();
        observed
            .recv_timeout(Duration::from_secs(5))
            .expect("active scan reached its blocked completion notification");
        let interrupted = service.inner.scan_cancel.lock().unwrap().clone();
        let query = service.begin_search("main", "battery-query").unwrap();
        battery.store(true, Ordering::Release);
        wait_until("power observer cancels while scan is still blocked", || {
            interrupted.load(Ordering::Acquire)
        });
        assert!(
            !query.load(Ordering::Acquire),
            "battery policy must not cancel a user's search"
        );
        assert_eq!(
            service
                .search("retainedbatterycanary", "", None, &query)
                .unwrap()
                .hits
                .len(),
            1
        );
        assert_eq!(
            fs::read_to_string(&busy).unwrap(),
            "resumedbatterycanary interrupted original"
        );
        release.send(()).unwrap();
        wait_until(
            "battery pause is published after the cancelled work returns",
            || service.status().unwrap().pause_reason.as_deref() == Some("battery"),
        );
        assert_eq!(service.status().unwrap().phase, "paused");
        assert_eq!(
            service
                .search("resumedbatterycanary", "", None, &query)
                .unwrap()
                .hits
                .len(),
            1,
            "completed documents survive a battery interruption"
        );
        battery.store(false, Ordering::Release);
        wait_until("AC resumes the complete scan", || {
            let s = service.status().unwrap();
            s.phase == "ready" && s.indexed == 2
        });
        assert!(!Arc::ptr_eq(
            &interrupted,
            &service.inner.scan_cancel.lock().unwrap()
        ));
        assert_eq!(
            service
                .search("resumedbatterycanary", "", None, &query)
                .unwrap()
                .hits
                .len(),
            1
        );
        assert_eq!(
            service
                .search("retainedbatterycanary", "", None, &query)
                .unwrap()
                .hits
                .len(),
            1
        );
        assert!(!query.load(Ordering::Acquire));
        service.shutdown();
    }

    #[test]
    fn manual_pause_survives_power_changes_and_disabling_battery_pause_releases_pending_work() {
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("sources");
        fs::create_dir(&folder).unwrap();
        fs::write(folder.join("sample.md"), "batterysettingscanary").unwrap();
        let battery = Arc::new(AtomicU64::new(1));
        let sample = battery.clone();
        let service = ContentService::open_with_power_reader(
            &root.path().join("cache"),
            Err("no model".into()),
            |_| {},
            Arc::new(move || match sample.load(Ordering::Acquire) {
                0 => Some(false),
                1 => Some(true),
                _ => None,
            }),
            Duration::from_millis(20),
        )
        .unwrap();
        let scope = SearchScope {
            id: "docs".into(),
            name: "docs".into(),
            path: folder.to_string_lossy().into(),
            filename_enabled: true,
            content_enabled: true,
            external_allowed: false,
        };
        service.set_battery_pause(true);
        service.configure(vec![scope]).unwrap();
        wait_until("queued work is held on battery", || {
            service.status().unwrap().pause_reason.as_deref() == Some("battery")
        });
        service.refresh().unwrap();
        wait_until("refresh while held must republish the paused phase", || {
            service.status().unwrap().phase == "paused"
        });
        service.set_battery_pause(false);
        wait_until("switching the preference off releases pending work", || {
            let status = service.status().unwrap();
            status.phase == "ready" && status.indexed == 1
        });
        assert_eq!(service.status().unwrap().indexed, 1);
        service.set_battery_pause(true);
        service.refresh().unwrap();
        wait_until("battery hold returns", || {
            service.status().unwrap().pause_reason.as_deref() == Some("battery")
        });
        service.cancel_index();
        battery.store(0, Ordering::Release);
        thread::sleep(Duration::from_millis(400));
        assert_eq!(service.status().unwrap().phase, "cancelled");
        assert_eq!(service.status().unwrap().pause_reason, None);
        assert!(service.inner.paused.load(Ordering::Acquire));
        battery.store(2, Ordering::Release);
        service.refresh().unwrap();
        wait_until(
            "unknown power does not suspend explicit resumed work",
            || {
                let status = service.status().unwrap();
                status.phase == "ready" && status.indexed == 1
            },
        );
        assert_eq!(service.status().unwrap().indexed, 1);
        service.shutdown();
    }

    #[test]
    fn password_retry_preserves_pause_cache_and_current_authorization() {
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("source");
        fs::create_dir(&folder).unwrap();
        let path = folder.join("암호.pdf");
        let bytes = include_bytes!("../../tests/fixtures/encrypted-pdf/aes256.pdf");
        fs::write(&path, bytes).unwrap();
        let service =
            ContentService::open(&root.path().join("cache"), Err("no model".into()), |_| {})
                .unwrap();
        let scope = SearchScope {
            id: "docs".into(),
            name: "docs".into(),
            path: folder.to_string_lossy().into(),
            filename_enabled: true,
            content_enabled: true,
            external_allowed: false,
        };
        service.configure(vec![scope.clone()]).unwrap();
        let start = Instant::now();
        while service.status().unwrap().phase != "ready"
            && start.elapsed() < Duration::from_secs(10)
        {
            thread::sleep(Duration::from_millis(20));
        }
        service.cancel_index();
        let cancel = AtomicBool::new(false);
        let denied = service
            .unlock_pdf(&path, "wrong-fixture-password", &cancel)
            .unwrap_err();
        assert_eq!(denied.code, "passwordRejected");
        assert!(service
            .search("unlockcanary", "", None, &cancel)
            .unwrap()
            .hits
            .is_empty());
        let summary = service
            .unlock_pdf(&path, "암호-fixture-密碼", &cancel)
            .unwrap();
        assert_eq!(
            summary.extractor_version.as_deref(),
            Some(EXTRACTOR_VERSION)
        );
        assert_eq!(service.status().unwrap().phase, "cancelled");
        assert_eq!(
            service
                .search("unlockcanarythird", "", None, &cancel)
                .unwrap()
                .hits[0]
                .chunk
                .page,
            Some(3)
        );
        assert_eq!(fs::read(&path).unwrap(), bytes);
        let private_cache = folder.join("internal-cache");
        fs::create_dir(&private_cache).unwrap();
        service.exclude_paths(std::slice::from_ref(&private_cache));
        let private_pdf = private_cache.join("hidden.pdf");
        fs::write(&private_pdf, bytes).unwrap();
        assert_eq!(
            service
                .unlock_pdf(&private_pdf, "암호-fixture-密碼", &cancel)
                .unwrap_err()
                .code,
            "outsideScope"
        );
        let excluded = root.path().join("cache/hidden.pdf");
        fs::write(&excluded, bytes).unwrap();
        assert_eq!(
            service
                .unlock_pdf(&excluded, "암호-fixture-密碼", &cancel)
                .unwrap_err()
                .code,
            "outsideScope"
        );
        // Revocation between parsing and publication must deny the cached body.
        let extracted = document_extract::extract_document_with_password(
            &path,
            &ExtractOptions::default(),
            &cancel,
            "암호-fixture-密碼",
        )
        .unwrap();
        let mut disabled = scope.clone();
        disabled.content_enabled = false;
        service.configure(vec![disabled]).unwrap();
        assert_eq!(
            service
                .publish_unlocked(
                    &path,
                    &extracted,
                    &crate::ocr::Preferences::default(),
                    &cancel,
                    service.inner.policy_revision.load(Ordering::Acquire),
                )
                .unwrap_err()
                .code,
            "outsideScope"
        );
        service.configure(vec![scope.clone()]).unwrap();
        service.cancel_index();
        // Equal length and restored mtime must not bypass the final raw SHA.
        let before_policy = service.inner.policy_revision.load(Ordering::Acquire);
        service
            .configure_with_policy(
                vec![scope.clone()],
                Default::default(),
                crate::file_policy::Categories {
                    archives: false,
                    ..Default::default()
                },
            )
            .unwrap();
        service.cancel_index();
        assert_eq!(
            service
                .publish_unlocked(
                    &path,
                    &extracted,
                    &crate::ocr::Preferences::default(),
                    &cancel,
                    before_policy
                )
                .unwrap_err()
                .code,
            "cancelled"
        );
        service.configure(vec![scope]).unwrap();
        service.cancel_index();
        let modified = fs::metadata(&path).unwrap().modified().unwrap();
        let mut changed = bytes.to_vec();
        changed[0] = b'!';
        fs::write(&path, changed).unwrap();
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
        assert_eq!(
            service
                .publish_unlocked(
                    &path,
                    &extracted,
                    &crate::ocr::Preferences::default(),
                    &cancel,
                    service.inner.policy_revision.load(Ordering::Acquire),
                )
                .unwrap_err()
                .code,
            "sourceChanged"
        );
        service.shutdown();
    }
    #[test]
    fn linked_scope_roots_are_reported_without_indexing_the_target() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("target");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("secret.md"), "privatecanary").unwrap();
        let link = directory.path().join("link");
        crate::path_identity::junction(&link, &target);
        let service = ContentService::open(
            &directory.path().join("index"),
            Err("not installed".into()),
            |_| {},
        )
        .unwrap();
        service
            .configure(vec![SearchScope {
                id: "linked".into(),
                name: "linked".into(),
                path: link.to_string_lossy().into(),
                filename_enabled: true,
                content_enabled: true,
                external_allowed: false,
            }])
            .unwrap();
        wait_ready(&service);
        let status = service.status().unwrap();
        assert_eq!(status.indexed, 0);
        assert!(status.issues.iter().any(|issue| issue.code == "unsafePath"));
        assert!(service
            .search("privatecanary", "", None, &AtomicBool::new(false))
            .unwrap()
            .hits
            .is_empty());
        service.shutdown();
    }
    #[test]
    fn keyword_snapshot_checks_revision_scope_and_mid_stream_changes() {
        use crate::content_store::SnapshotError;
        let directory = tempfile::tempdir().unwrap();
        let docs = directory.path().join("docs");
        fs::create_dir(&docs).unwrap();
        fs::write(docs.join("a.md"), "snapshotneedle").unwrap();
        let service = ContentService::open(
            &directory.path().join("data"),
            Err("not configured".into()),
            |_| {},
        )
        .unwrap();
        service
            .configure(vec![SearchScope {
                id: "allowed".into(),
                name: "Allowed".into(),
                path: docs.to_string_lossy().into(),
                filename_enabled: true,
                content_enabled: true,
                external_allowed: false,
            }])
            .unwrap();
        wait_ready(&service);
        let mut search = KeywordSearch {
            query: "snapshotneedle".into(),
            extension: "md".into(),
            scope_id: Some("allowed".into()),
            content_revision: service.status().unwrap().revision,
            settings_revision: 0,
        };
        let mut count = 0;
        assert!(!service
            .snapshot_keyword_files(&search, &AtomicBool::new(false), &mut |_| {
                count += 1;
                Ok::<_, ()>(())
            })
            .unwrap());
        assert_eq!(count, 1);
        search.content_revision += 1;
        assert!(
            matches!(service.snapshot_keyword_files(&search, &AtomicBool::new(false), &mut |_| Ok::<_, ()>(())), Err(SnapshotError::Source(ContentError {code, ..})) if code=="sourceChanged")
        );
        search.content_revision -= 1;
        assert!(
            matches!(service.snapshot_keyword_files(&search, &AtomicBool::new(false), &mut |_| { service.update(|_| {}); Ok::<_, ()>(()) }), Err(SnapshotError::Source(ContentError {code, ..})) if code=="sourceChanged")
        );
        search.content_revision = service.status().unwrap().revision;
        search.scope_id = Some("removed".into());
        let mut leaked = false;
        let result = service.snapshot_keyword_files(&search, &AtomicBool::new(false), &mut |_| {
            leaked = true;
            Ok::<_, ()>(())
        });
        assert!(!leaked);
        assert!(result.is_ok()); // A removed scope is an empty set, never all roots.
        service.shutdown();
    }
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
