use crate::filename_pattern::{initial_projection, FilenamePattern};
use crate::keyboard_pattern::{keyboard_projection, KeyboardPattern};
use crate::settings::SearchScope;
use chrono::{Datelike, Local, NaiveDate, TimeZone};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use std::{
    cmp::Ordering as CmpOrdering,
    collections::{BTreeMap, BinaryHeap, HashSet, VecDeque},
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Arc, Condvar, Mutex, RwLock,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use unicode_normalization::UnicodeNormalization;
use walkdir::WalkDir;

fn normalized(value: &str) -> String {
    value
        .nfc()
        .collect::<String>()
        .to_lowercase()
        .replace('\\', "/")
}
fn path_key(path: &Path) -> String {
    let value = path.to_string_lossy();
    let value = value
        .strip_prefix(r"\\?\UNC\")
        .map(|tail| format!(r"\\{tail}"))
        .unwrap_or_else(|| value.strip_prefix(r"\\?\").unwrap_or(&value).to_string());
    #[cfg(windows)]
    let value = value.to_lowercase();
    // Keep filesystem identity distinct from the NFC-normalized search text.
    value.replace('\\', "/").trim_end_matches('/').to_string()
}
fn under(path: &str, root: &str) -> bool {
    path == root
        || path
            .strip_prefix(root)
            .is_some_and(|tail| tail.starts_with('/'))
}
pub(crate) enum SnapshotError<E> {
    Index(String),
    Visitor(E),
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FilenameSearch {
    pub query: String,
    pub extension: String,
    pub scope_id: Option<String>,
    pub index_revision: u64,
    pub settings_revision: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileResult {
    pub path: String,
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified_ms: Option<i64>,
}
/// One indexed path. Derived strings are stored once: the normalized name, extension and
/// initials of the name are slices of the normalized path, so each entry keeps two strings
/// (display path and identity key) plus optional projections only when they differ.
struct Entry {
    path: Box<str>,
    key: Box<str>,
    /// NFC/lowercase search text when it differs from `key` (usually identical on Windows).
    search: Option<Box<str>>,
    /// Byte offset of the file name inside the search text and its initials projection.
    name_start: u32,
    initials_path: Option<Box<str>>,
    keyboard_path: Option<Box<str>>,
    is_dir: bool,
    size: u64,
    modified_ms: Option<i64>,
    seen: u64,
}
impl Entry {
    fn new(raw: Raw, seen: u64) -> Self {
        // Most system paths are ASCII. Avoid NFC iteration and allocating two
        // Hangul projections for every such path on a whole-drive scan.
        let ascii = raw.key.is_ascii();
        let search = if ascii {
            raw.key.to_ascii_lowercase()
        } else {
            normalized(&raw.key)
        };
        let name = if ascii {
            raw.name.to_ascii_lowercase()
        } else {
            normalized(&raw.name)
        };
        // Initial projection keeps UTF-8 byte lengths (3-byte syllable → 3-byte jamo), so the
        // name offset is valid for both strings.
        let name_start = if search.ends_with(&name) {
            search.len() - name.len()
        } else {
            search.rfind('/').map_or(0, |index| index + 1)
        } as u32;
        Self {
            path: display_path(&raw.path).into(),
            initials_path: (!ascii)
                .then(|| initial_projection(&search))
                .flatten()
                .map(Into::into),
            keyboard_path: (!ascii)
                .then(|| keyboard_projection(&search))
                .flatten()
                .map(Into::into),
            search: (search != raw.key).then(|| search.into()),
            key: raw.key.into(),
            name_start,
            is_dir: raw.is_dir,
            size: raw.size,
            modified_ms: raw.modified_ms,
            seen,
        }
    }
    fn searchable(&self) -> &str {
        self.search.as_deref().unwrap_or(&self.key)
    }
    fn name(&self) -> &str {
        &self.searchable()[self.name_start as usize..]
    }
    fn initials_name(&self) -> Option<&str> {
        self.initials_path
            .as_deref()
            .and_then(|initials| initials.get(self.name_start as usize..))
    }
    fn extension(&self) -> &str {
        let name = self.name();
        match name.rfind('.') {
            Some(index) if index > 0 => &name[index + 1..],
            _ => "",
        }
    }
    fn unchanged(&self, raw: &Raw) -> bool {
        self.is_dir == raw.is_dir
            && self.size == raw.size
            && self.modified_ms == raw.modified_ms
            && *self.path == *display_path(&raw.path)
    }
    fn result(&self) -> FileResult {
        let name = self
            .path
            .rsplit(['\\', '/'])
            .find(|part| !part.is_empty())
            .unwrap_or(&self.path);
        FileResult {
            path: self.path.to_string(),
            name: name.to_string(),
            is_dir: self.is_dir,
            size: self.size,
            modified_ms: self.modified_ms,
        }
    }
}
/// Metadata collected by the walker before deciding whether an entry must be rebuilt.
struct Raw {
    path: PathBuf,
    key: String,
    name: String,
    is_dir: bool,
    size: u64,
    modified_ms: Option<i64>,
}
struct WalkJobs {
    paths: VecDeque<(PathBuf, bool)>,
    outstanding: usize,
}
/// Entries at `root` and below it, using the sorted key order ('0' follows '/').
fn subtree<'a>(
    entries: &'a BTreeMap<Box<str>, Entry>,
    root: &str,
) -> impl Iterator<Item = &'a Entry> + 'a {
    let (low, high) = (format!("{root}/"), format!("{root}0"));
    entries.get(root).into_iter().chain(
        entries
            .range::<str, _>((
                std::ops::Bound::Included(low.as_str()),
                std::ops::Bound::Excluded(high.as_str()),
            ))
            .map(|(_, entry)| entry),
    )
}
#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexStatus {
    pub scanning: bool,
    pub count: usize,
    pub revision: u64,
    pub skipped: usize,
    pub warnings: Vec<String>,
    pub watching: bool,
}
#[derive(Clone)]
struct Root {
    id: String,
    key: String,
    path: PathBuf,
    /// The scope path as configured, used to tell whether a reconfigured scope is unchanged.
    source: String,
}
#[derive(Default)]
struct Catalog {
    entries: BTreeMap<Box<str>, Entry>,
    roots: Vec<Root>,
    status: IndexStatus,
}

/// Bound queued *paths*, not notifications: a downloading file can emit tens
/// of thousands of writes without requiring another walk of the whole drive.
#[derive(Default)]
struct PendingPaths {
    paths: BTreeMap<String, PathBuf>,
    overflow: bool,
}
impl PendingPaths {
    fn push(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        for path in paths {
            let key = path_key(&path);
            if self.paths.contains_key(&key) {
                continue;
            }
            if self.paths.len() == 4096 {
                self.overflow = true;
                break;
            }
            self.paths.insert(key, path);
        }
    }
    fn take(&mut self) -> (bool, Vec<PathBuf>) {
        let overflow = std::mem::take(&mut self.overflow);
        (overflow, minimal_paths(std::mem::take(&mut self.paths)))
    }
}

/// Keep every scope in the catalog for filtering, but visit/watch nested roots
/// only once. Checking ancestors also handles sibling prefixes such as a-b.
fn minimal_paths(paths: BTreeMap<String, PathBuf>) -> Vec<PathBuf> {
    paths
        .iter()
        .filter(|(key, _)| {
            let mut parent = key.as_str();
            while let Some((head, _)) = parent.rsplit_once('/') {
                if paths.contains_key(head) {
                    return false;
                }
                parent = head;
            }
            true
        })
        .map(|(_, path)| path.clone())
        .collect()
}

fn root_paths(roots: &[Root]) -> Vec<PathBuf> {
    minimal_paths(
        roots
            .iter()
            .map(|root| (root.key.clone(), root.path.clone()))
            .collect(),
    )
}
#[derive(Clone)]
pub struct FileIndex {
    catalog: Arc<RwLock<Catalog>>,
    generation: Arc<AtomicU64>,
    tx: mpsc::Sender<(u64, Vec<SearchScope>)>,
    lifetime: Arc<()>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResponse {
    pub results: Vec<FileResult>,
    pub total: usize,
    pub status: IndexStatus,
    pub keyboard_corrected: bool,
}

impl FileIndex {
    pub fn new() -> Self {
        let catalog = Arc::new(RwLock::new(Catalog::default()));
        let generation = Arc::new(AtomicU64::new(0));
        let (tx, rx) = mpsc::channel();
        let this = Self {
            catalog,
            generation,
            tx,
            lifetime: Arc::new(()),
        };
        let worker = this.clone();
        std::thread::spawn(move || worker.run(rx));
        this
    }
    pub fn configure(&self, scopes: Vec<SearchScope>) {
        self.request_refresh(scopes, true);
    }
    pub fn refresh(&self, scopes: Vec<SearchScope>) {
        self.request_refresh(scopes, false);
    }
    fn request_refresh(&self, scopes: Vec<SearchScope>, clear: bool) {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        if let Ok(mut catalog) = self.catalog.write() {
            if self.generation.load(Ordering::SeqCst) != generation {
                return;
            }
            // Removed or disabled scopes must stop being searchable immediately. Entries under
            // scopes that are still configured unchanged stay visible while the rescan runs, so
            // adding a folder does not blank existing results.
            if clear {
                let kept: Vec<Root> = catalog
                    .roots
                    .iter()
                    .filter(|root| {
                        scopes.iter().any(|scope| {
                            scope.filename_enabled
                                && scope.id == root.id
                                && scope.path == root.source
                        })
                    })
                    .cloned()
                    .collect();
                let before = catalog.entries.len();
                catalog
                    .entries
                    .retain(|key, _| kept.iter().any(|root| under(key, &root.key)));
                if catalog.entries.len() != before || kept.len() != catalog.roots.len() {
                    catalog.roots = kept;
                }
            }
            catalog.status = IndexStatus {
                scanning: true,
                count: catalog.entries.len(),
                revision: catalog.status.revision + 1,
                ..Default::default()
            };
        }
        let _ = self.tx.send((generation, scopes));
    }
    pub fn status(&self) -> Result<IndexStatus, String> {
        self.catalog
            .read()
            .map(|catalog| catalog.status.clone())
            .map_err(|_| "File index unavailable.".into())
    }
    fn current(&self, generation: u64) -> bool {
        Arc::strong_count(&self.lifetime) > 1
            && self.generation.load(Ordering::SeqCst) == generation
    }
    fn run(&self, rx: mpsc::Receiver<(u64, Vec<SearchScope>)>) {
        let pending = Arc::new(Mutex::new(PendingPaths::default()));
        let overflow = Arc::new(AtomicBool::new(false));
        let mut watcher: Option<RecommendedWatcher> = None;
        let mut generation = 0;
        let mut scopes = vec![];
        let mut last_reconcile = SystemTime::now();
        // Each full walk gets a new sweep id; entries not seen by the latest sweep are removed.
        let mut sweep = 0u64;
        loop {
            if Arc::strong_count(&self.lifetime) == 1 {
                return;
            }
            match rx.recv_timeout(Duration::from_millis(350)) {
                Ok(mut request) => {
                    while let Ok(next) = rx.try_recv() {
                        if next.0 > request.0 {
                            request = next;
                        }
                    }
                    generation = request.0;
                    scopes = request.1;
                    watcher.take();
                    pending
                        .lock()
                        .map(|mut paths| *paths = PendingPaths::default())
                        .ok();
                    overflow.store(false, Ordering::SeqCst);
                    let mut roots = vec![];
                    let mut warnings = vec![];
                    for scope in scopes.iter().filter(|scope| scope.filename_enabled) {
                        match fs::canonicalize(&scope.path) {
                            Ok(path) if path.is_dir() => roots.push(Root {
                                id: scope.id.clone(),
                                key: path_key(&path),
                                path,
                                source: scope.path.clone(),
                            }),
                            _ => warnings.push(format!("Folder unavailable: {}", scope.path)),
                        }
                    }
                    let queue = pending.clone();
                    let dropped = overflow.clone();
                    match notify::recommended_watcher(
                        move |event: notify::Result<notify::Event>| match event {
                            Ok(event) if !matches!(event.kind, EventKind::Access(_)) => {
                                if event.need_rescan() {
                                    dropped.store(true, Ordering::SeqCst);
                                }
                                if let Ok(mut paths) = queue.lock() {
                                    paths.push(event.paths);
                                }
                            }
                            Err(_) => {
                                dropped.store(true, Ordering::SeqCst);
                            }
                            _ => {}
                        },
                    ) {
                        Ok(mut next) => {
                            for path in root_paths(&roots) {
                                if let Err(error) = next.watch(&path, RecursiveMode::Recursive) {
                                    warnings.push(format!(
                                        "Live updates unavailable for {}: {error}",
                                        path.display()
                                    ));
                                }
                            }
                            watcher = Some(next);
                        }
                        Err(error) => warnings.push(format!("Live updates unavailable: {error}")),
                    }
                    if !self.current(generation) {
                        continue;
                    }
                    if let Ok(mut catalog) = self.catalog.write() {
                        catalog.roots = roots.clone();
                        catalog.status.watching = watcher.is_some() && warnings.is_empty();
                        catalog.status.warnings = warnings.into_iter().take(5).collect();
                    }
                    sweep += 1;
                    self.scan(generation, &root_paths(&roots), sweep, true);
                    last_reconcile = SystemTime::now();
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            if generation == 0 || !self.current(generation) {
                continue;
            }
            // Periodic reconciliation repairs missed network / OS watch events. A healthy watcher is
            // kept running and only the tree is re-walked (unchanged entries are not rebuilt and do
            // not bump the revision). Missing roots or failed watches need a full restart instead.
            let (healthy, roots) = match self.catalog.read() {
                Ok(catalog) => (
                    catalog.status.warnings.is_empty() && watcher.is_some(),
                    catalog.roots.clone(),
                ),
                Err(_) => continue,
            };
            let interval = Duration::from_secs(if healthy { 300 } else { 120 });
            let (queue_overflow, paths) = match pending.lock() {
                Ok(mut paths) => paths.take(),
                Err(_) => continue,
            };
            let overflowed = overflow.swap(false, Ordering::SeqCst) || queue_overflow;
            if overflowed || last_reconcile.elapsed().unwrap_or_default() > interval {
                if !healthy {
                    self.refresh(scopes.clone());
                    continue;
                }
                sweep += 1;
                self.scan(generation, &root_paths(&roots), sweep, true);
                last_reconcile = SystemTime::now();
                continue;
            }
            if paths.is_empty() {
                continue;
            }
            let roots = match self.catalog.read() {
                Ok(catalog) => catalog.roots.clone(),
                Err(_) => continue,
            };
            let unique: Vec<_> = paths
                .into_iter()
                .filter(|path| roots.iter().any(|root| under(&path_key(path), &root.key)))
                .collect();
            if !unique.is_empty() {
                // Re-walk only the changed subtrees under a fresh sweep id, then drop entries in
                // those subtrees that the walk no longer saw (deleted or renamed away).
                sweep += 1;
                self.scan(generation, &unique, sweep, false);
                if let Ok(mut catalog) = self.catalog.write() {
                    if !self.current(generation) {
                        continue;
                    }
                    let stale: Vec<Box<str>> = unique
                        .iter()
                        .flat_map(|path| {
                            subtree(&catalog.entries, &path_key(path))
                                .filter(|entry| entry.seen != sweep)
                                .map(|entry| entry.key.clone())
                                .collect::<Vec<_>>()
                        })
                        .collect();
                    for key in &stale {
                        catalog.entries.remove(key);
                    }
                    if !stale.is_empty() {
                        catalog.status.revision += 1;
                        catalog.status.count = catalog.entries.len();
                    }
                }
            }
        }
    }
    /// Walks `paths`. A reconcile sweep (`reconcile`) also removes entries the walk did not see.
    fn scan(&self, generation: u64, paths: &[PathBuf], sweep: u64, reconcile: bool) {
        if !self.current(generation) {
            return;
        }
        if let Ok(mut catalog) = self.catalog.write() {
            catalog.status.scanning = true;
            if reconcile {
                catalog.status.skipped = 0;
            }
        }
        // A single recursive walk serializes directory reads, Unicode work and
        // catalog insertion. Disjoint directory jobs overlap reads while each
        // worker keeps only a small publication batch. Nested input roots have
        // already been coalesced, so no two jobs enumerate the same directory.
        let jobs = Mutex::new(WalkJobs {
            paths: paths.iter().cloned().map(|path| (path, true)).collect(),
            outstanding: paths.len(),
        });
        let wake = Condvar::new();
        let threads = std::thread::available_parallelism()
            .map_or(2, usize::from)
            .clamp(1, 8);
        std::thread::scope(|scope| {
            for _ in 0..threads {
                let jobs = &jobs;
                let wake = &wake;
                scope.spawn(move || {
                    let mut batch = Vec::with_capacity(512);
                    let mut skipped = 0;
                    loop {
                        let work = {
                            let mut jobs = jobs.lock().unwrap_or_else(|error| error.into_inner());
                            loop {
                                if !self.current(generation) || jobs.outstanding == 0 {
                                    break None;
                                }
                                if let Some(path) = jobs.paths.pop_front() {
                                    break Some(path);
                                }
                                jobs = wake
                                    .wait_timeout(jobs, Duration::from_millis(100))
                                    .unwrap_or_else(|error| error.into_inner())
                                    .0;
                            }
                        };
                        let Some((path, include_root)) = work else {
                            break;
                        };
                        let children = self.walk_directory(
                            generation,
                            &path,
                            include_root,
                            sweep,
                            &mut batch,
                            &mut skipped,
                        );
                        let mut jobs = jobs.lock().unwrap_or_else(|error| error.into_inner());
                        jobs.outstanding += children.len();
                        jobs.outstanding -= 1;
                        jobs.paths
                            .extend(children.into_iter().map(|path| (path, false)));
                        wake.notify_all();
                    }
                    self.publish(generation, sweep, &mut batch, skipped, false);
                });
            }
        });
        self.publish(generation, sweep, &mut vec![], 0, !reconcile);
        if reconcile {
            if let Ok(mut catalog) = self.catalog.write() {
                if !self.current(generation) {
                    return;
                }
                let before = catalog.entries.len();
                catalog.entries.retain(|_, entry| entry.seen == sweep);
                if catalog.entries.len() != before {
                    catalog.status.revision += 1;
                }
                catalog.status.count = catalog.entries.len();
                catalog.status.scanning = false;
            }
        }
    }
    fn walk_directory(
        &self,
        generation: u64,
        path: &Path,
        include_root: bool,
        sweep: u64,
        batch: &mut Vec<Raw>,
        skipped: &mut usize,
    ) -> Vec<PathBuf> {
        let mut children = vec![];
        // Depth one makes directory ownership explicit. Keep WalkDir's cached
        // Windows metadata and the existing symlink/junction safeguards.
        let walker = WalkDir::new(path)
            .max_depth(1)
            .follow_links(false)
            .follow_root_links(false)
            .into_iter()
            .filter_entry(|entry| {
                if entry.file_type().is_symlink() {
                    return false;
                }
                #[cfg(windows)]
                {
                    use std::os::windows::fs::MetadataExt;
                    if entry.file_type().is_dir()
                        && entry
                            .metadata()
                            .is_ok_and(|metadata| metadata.file_attributes() & 0x400 != 0)
                        && !fs::canonicalize(entry.path())
                            .is_ok_and(|target| path_key(&target) == path_key(entry.path()))
                    {
                        // Do not descend into junctions or other directory aliases. Cloud placeholders
                        // whose canonical location is unchanged can still contribute their filenames.
                        return false;
                    }
                }
                true
            });
        for entry in walker {
            if !self.current(generation) {
                return children;
            }
            match entry {
                Ok(entry) => {
                    if entry.depth() == 0 && !include_root {
                        continue;
                    }
                    if !entry.file_type().is_dir() && !entry.file_type().is_file() {
                        continue;
                    }
                    let metadata = match entry.metadata() {
                        Ok(metadata) => metadata,
                        Err(_) => {
                            *skipped += 1;
                            continue;
                        }
                    };
                    if entry.depth() > 0 && metadata.is_dir() {
                        children.push(entry.path().to_owned());
                    }
                    batch.push(Raw {
                        key: path_key(entry.path()),
                        name: entry.file_name().to_string_lossy().to_string(),
                        is_dir: metadata.is_dir(),
                        size: if metadata.is_dir() { 0 } else { metadata.len() },
                        modified_ms: metadata
                            .modified()
                            .ok()
                            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                            .map(|duration| duration.as_millis() as i64),
                        path: entry.into_path(),
                    });
                    if batch.len() >= 512 {
                        self.publish(generation, sweep, batch, *skipped, false);
                        *skipped = 0;
                    }
                }
                Err(_) => *skipped += 1,
            }
        }
        children
    }
    /// Inserts a batch. Unchanged entries only get their sweep id refreshed, so a reconcile over
    /// an unchanged tree rebuilds nothing and does not bump the revision (no UI re-search).
    fn publish(
        &self,
        generation: u64,
        sweep: u64,
        batch: &mut Vec<Raw>,
        skipped: usize,
        done: bool,
    ) {
        enum Update {
            Seen(String),
            Replace(Entry),
        }
        let candidates: Vec<_> = match self.catalog.read() {
            Ok(catalog) => batch
                .drain(..)
                .map(|raw| {
                    let unchanged = catalog
                        .entries
                        .get(raw.key.as_str())
                        .is_some_and(|entry| entry.unchanged(&raw));
                    (raw, unchanged)
                })
                .collect(),
            Err(_) => return,
        };
        // Derive Unicode search data outside the catalog lock. Search/status
        // readers and other directory workers no longer wait on this CPU work.
        let updates: Vec<_> = candidates
            .into_iter()
            .map(|(raw, unchanged)| {
                if unchanged {
                    Update::Seen(raw.key)
                } else {
                    Update::Replace(Entry::new(raw, sweep))
                }
            })
            .collect();
        if let Ok(mut catalog) = self.catalog.write() {
            if !self.current(generation) {
                return;
            }
            let mut changed = false;
            for update in updates {
                match update {
                    Update::Seen(key) => {
                        if let Some(entry) = catalog.entries.get_mut(key.as_str()) {
                            entry.seen = sweep;
                        }
                    }
                    Update::Replace(entry) => {
                        catalog.entries.insert(entry.key.clone(), entry);
                        changed = true;
                    }
                }
            }
            catalog.status.count = catalog.entries.len();
            catalog.status.skipped += skipped;
            if changed {
                catalog.status.revision += 1;
            }
            catalog.status.scanning = !done;
        }
    }
    #[cfg(test)]
    pub fn search(
        &self,
        query: &str,
        extension: &str,
        scope_id: Option<&str>,
        limit: usize,
    ) -> Result<SearchResponse, String> {
        self.search_with_keyboard(query, extension, scope_id, limit, true)
    }
    pub fn search_with_keyboard(
        &self,
        query: &str,
        extension: &str,
        scope_id: Option<&str>,
        limit: usize,
        allow_keyboard: bool,
    ) -> Result<SearchResponse, String> {
        let query = Query::parse(query, extension)?;
        let catalog = self.catalog.read().map_err(|_| "File index unavailable.")?;
        let limit = limit.clamp(1, 500);
        // A selected scope only walks its own key range instead of filtering every entry.
        let candidates: Vec<&Entry> = match scope_id {
            None => catalog.entries.values().collect(),
            Some(id) => match catalog.roots.iter().find(|root| root.id == id) {
                Some(root) => subtree(&catalog.entries, &root.key).collect(),
                None => vec![],
            },
        };
        let (mut total, mut ranked) = top_matches(&candidates, limit, |entry| {
            query.matches(entry).then(|| query.rank(entry))
        });
        let mut keyboard_corrected = false;
        if total == 0 && allow_keyboard && query.keyboard_terms.iter().any(Option::is_some) {
            (total, ranked) = top_matches(&candidates, limit, |entry| {
                query.matches_keyboard(entry).then(|| {
                    if query.keyboard_name_matches(entry) {
                        6
                    } else {
                        7
                    }
                })
            });
            keyboard_corrected = total > 0;
        }
        Ok(SearchResponse {
            keyboard_corrected,
            results: ranked
                .into_iter()
                .map(|ranked| ranked.entry.result())
                .collect(),
            total,
            status: catalog.status.clone(),
        })
    }
    pub fn resolve_result(&self, path: &str) -> Result<PathBuf, String> {
        let canonical =
            fs::canonicalize(path).map_err(|_| "This file no longer exists or is unavailable.")?;
        let key = path_key(&canonical);
        let catalog = self.catalog.read().map_err(|_| "File index unavailable.")?;
        if key != path_key(Path::new(path))
            || !catalog.entries.contains_key(key.as_str())
            || !catalog.roots.iter().any(|root| under(&key, &root.key))
        {
            return Err(
                "This path is outside the active file index. Refresh and search again.".into(),
            );
        }
        Ok(canonical)
    }
    /// Stream all matching files from one catalog revision. This deliberately
    /// does not reuse the UI's top-200 list or allocate a second full result set.
    pub(crate) fn snapshot_files<E>(
        &self,
        search: &FilenameSearch,
        allow_keyboard: bool,
        cancel: &AtomicBool,
        visit: &mut dyn FnMut(&str) -> Result<(), E>,
    ) -> Result<bool, SnapshotError<E>> {
        let query = Query::parse(&search.query, &search.extension).map_err(SnapshotError::Index)?;
        let catalog = self
            .catalog
            .read()
            .map_err(|_| SnapshotError::Index("File index unavailable.".into()))?;
        if catalog.status.revision != search.index_revision {
            return Err(SnapshotError::Index("Search results changed.".into()));
        }
        let root = search
            .scope_id
            .as_deref()
            .and_then(|id| catalog.roots.iter().find(|root| root.id == id));
        let in_scope = |entry: &&Entry| {
            search.scope_id.is_none() || root.is_some_and(|root| under(&entry.key, &root.key))
        };
        let literal = catalog
            .entries
            .values()
            .filter(in_scope)
            .any(|entry| query.matches(entry));
        let corrected =
            !literal && allow_keyboard && query.keyboard_terms.iter().any(Option::is_some);
        for entry in catalog.entries.values().filter(in_scope) {
            if cancel.load(Ordering::Acquire) {
                return Err(SnapshotError::Index("Search cancelled.".into()));
            }
            if !entry.is_dir
                && if corrected {
                    query.matches_keyboard(entry)
                } else {
                    query.matches(entry)
                }
            {
                visit(&entry.path).map_err(SnapshotError::Visitor)?;
            }
        }
        Ok(catalog.status.scanning
            || catalog.status.skipped > 0
            || !catalog.status.warnings.is_empty())
    }
}
fn display_path(path: &Path) -> String {
    let value = path.to_string_lossy();
    if let Some(tail) = value.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{tail}")
    } else {
        value.strip_prefix(r"\\?\").unwrap_or(&value).to_string()
    }
}
struct Ranked<'a> {
    rank: u8,
    entry: &'a Entry,
}
impl PartialEq for Ranked<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == CmpOrdering::Equal
    }
}
impl Eq for Ranked<'_> {}
impl PartialOrd for Ranked<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<CmpOrdering> {
        Some(self.cmp(other))
    }
}
impl Ord for Ranked<'_> {
    fn cmp(&self, other: &Self) -> CmpOrdering {
        (self.rank, self.entry.name(), &self.entry.key).cmp(&(
            other.rank,
            other.entry.name(),
            &other.entry.key,
        ))
    }
}
/// Counts matches and keeps the best `limit` by rank. Large candidate sets are split across
/// threads (the per-entry substring tests dominate search time).
fn top_matches<'a>(
    candidates: &[&'a Entry],
    limit: usize,
    rank: impl Fn(&Entry) -> Option<u8> + Sync,
) -> (usize, Vec<Ranked<'a>>) {
    let scan = |chunk: &[&'a Entry]| {
        let mut heap = BinaryHeap::with_capacity(limit + 1);
        let mut total = 0;
        for &entry in chunk {
            if let Some(rank) = rank(entry) {
                total += 1;
                heap.push(Ranked { rank, entry });
                if heap.len() > limit {
                    heap.pop();
                }
            }
        }
        (total, heap)
    };
    let threads = std::thread::available_parallelism()
        .map_or(1, |count| count.get())
        .min(8);
    let (total, heap) = if candidates.len() < 20_000 || threads < 2 {
        scan(candidates)
    } else {
        let size = candidates.len().div_ceil(threads);
        std::thread::scope(|scope| {
            let workers: Vec<_> = candidates
                .chunks(size)
                .map(|chunk| scope.spawn(move || scan(chunk)))
                .collect();
            let mut total = 0;
            let mut merged = BinaryHeap::with_capacity(limit * threads);
            for worker in workers {
                let (count, heap) = worker.join().unwrap_or_default();
                total += count;
                for ranked in heap {
                    merged.push(ranked);
                    if merged.len() > limit {
                        merged.pop();
                    }
                }
            }
            (total, merged)
        })
    };
    (total, heap.into_sorted_vec())
}
struct Query {
    terms: Vec<FilenamePattern>,
    extensions: Vec<HashSet<String>>,
    dates: Vec<(i64, i64)>,
    exact: String,
    keyboard_terms: Vec<Option<KeyboardPattern>>,
}
impl Query {
    fn parse(query: &str, extension: &str) -> Result<Self, String> {
        if query.len() > 4096 {
            return Err("Search input is too long (4096 bytes maximum).".into());
        }
        let mut parsed = Self {
            terms: vec![],
            extensions: vec![],
            dates: vec![],
            exact: normalized(query.trim()),
            keyboard_terms: vec![],
        };
        if !extension.trim().is_empty() {
            parsed.extensions.push(Self::extensions(extension)?);
        }
        let mut token = String::new();
        let mut quoted = false;
        let mut tokens = vec![];
        for ch in query.chars() {
            if ch == '"' {
                quoted = !quoted;
            } else if ch.is_whitespace() && !quoted {
                if !token.is_empty() {
                    tokens.push(std::mem::take(&mut token));
                }
            } else {
                token.push(ch);
            }
        }
        if quoted {
            return Err("Close the double quote to finish the search phrase.".into());
        }
        if !token.is_empty() {
            tokens.push(token);
        }
        for token in tokens {
            let lower = normalized(&token);
            if let Some(ext) = lower
                .strip_prefix("extension:")
                .or_else(|| lower.strip_prefix("ext:"))
            {
                parsed.extensions.push(Self::extensions(ext)?);
            } else if let Some(date) = lower.strip_prefix("edit-date:") {
                parsed.dates.push(date_range(date)?);
            } else {
                parsed.terms.push(FilenamePattern::new(lower));
                parsed.keyboard_terms.push(KeyboardPattern::new(&token));
            }
        }
        parsed.exact = parsed
            .terms
            .iter()
            .map(FilenamePattern::text)
            .collect::<Vec<_>>()
            .join(" ");
        Ok(parsed)
    }
    fn extensions(value: &str) -> Result<HashSet<String>, String> {
        let items: HashSet<_> = value
            .split([',', ';', ' '])
            .map(|item| normalized(item.trim().trim_start_matches('.')))
            .filter(|item| !item.is_empty())
            .collect();
        if items.is_empty()
            || items.iter().any(|item| {
                !item
                    .chars()
                    .all(|ch| ch.is_alphanumeric() || ch == '_' || ch == '-')
            })
        {
            return Err("Enter an extension such as pdf, hwpx.".into());
        }
        Ok(items)
    }
    fn matches(&self, entry: &Entry) -> bool {
        self.terms
            .iter()
            .all(|term| term.matches(entry.searchable(), entry.initials_path.as_deref()))
            && self.filters_match(entry)
    }
    fn matches_keyboard(&self, entry: &Entry) -> bool {
        self.filters_match(entry)
            && self
                .terms
                .iter()
                .zip(&self.keyboard_terms)
                .all(|(term, keyboard)| {
                    term.matches(entry.searchable(), entry.initials_path.as_deref())
                        || keyboard.as_ref().is_some_and(|pattern| {
                            pattern.matches(entry.searchable(), entry.keyboard_path.as_deref())
                        })
                })
    }
    fn keyboard_name_matches(&self, entry: &Entry) -> bool {
        let projection = keyboard_projection(entry.name());
        self.terms
            .iter()
            .zip(&self.keyboard_terms)
            .all(|(term, keyboard)| {
                term.matches(entry.name(), entry.initials_name())
                    || keyboard
                        .as_ref()
                        .is_some_and(|pattern| pattern.matches(entry.name(), projection.as_deref()))
            })
    }
    fn filters_match(&self, entry: &Entry) -> bool {
        self.extensions
            .iter()
            .all(|set| !entry.is_dir && set.contains(entry.extension()))
            && self.dates.iter().all(|(start, end)| {
                entry
                    .modified_ms
                    .is_some_and(|time| time >= *start && time < *end)
            })
    }
    fn rank(&self, entry: &Entry) -> u8 {
        if entry.name() == self.exact {
            0
        } else if entry.name().starts_with(&self.exact) {
            1
        } else if self
            .terms
            .iter()
            .all(|term| term.literal_matches(entry.name()))
        {
            2
        } else if self
            .terms
            .iter()
            .all(|term| term.literal_matches(entry.searchable()))
        {
            3
        } else if self
            .terms
            .iter()
            .all(|term| term.matches(entry.name(), entry.initials_name()))
        {
            4
        } else {
            5
        }
    }
}
fn date_range(input: &str) -> Result<(i64, i64), String> {
    let (op, value) = [">=", "<=", ">", "<", "="]
        .into_iter()
        .find_map(|op| input.strip_prefix(op).map(|value| (op, value)))
        .unwrap_or(("=", input));
    let parts: Vec<_> = value.split('-').collect();
    let invalid = || "Use edit-date:>2023 or edit-date:>=2024-01-01.".to_string();
    if parts.is_empty() || parts.len() > 3 || parts[0].len() != 4 {
        return Err(invalid());
    }
    let year = parts[0].parse::<i32>().map_err(|_| invalid())?;
    if !(1970..=9998).contains(&year) {
        return Err(invalid());
    }
    let month = parts
        .get(1)
        .map(|value| value.parse::<u32>())
        .transpose()
        .map_err(|_| invalid())?
        .unwrap_or(1);
    let day = parts
        .get(2)
        .map(|value| value.parse::<u32>())
        .transpose()
        .map_err(|_| invalid())?
        .unwrap_or(1);
    let start = NaiveDate::from_ymd_opt(year, month, day).ok_or_else(invalid)?;
    let end = match parts.len() {
        1 => NaiveDate::from_ymd_opt(year + 1, 1, 1),
        2 => {
            if month == 12 {
                NaiveDate::from_ymd_opt(year + 1, 1, 1)
            } else {
                NaiveDate::from_ymd_opt(year, month + 1, 1)
            }
        }
        _ => start.succ_opt(),
    }
    .ok_or_else(invalid)?;
    let timestamp = |date: NaiveDate| {
        Local
            .with_ymd_and_hms(date.year(), date.month(), date.day(), 0, 0, 0)
            .earliest()
            .map(|date| date.timestamp_millis())
            .ok_or_else(invalid)
    };
    let (start, end) = (timestamp(start)?, timestamp(end)?);
    Ok(match op {
        ">" => (end, i64::MAX),
        ">=" => (start, i64::MAX),
        "<" => (i64::MIN, start),
        "<=" => (i64::MIN, end),
        _ => (start, end),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn scope(path: &Path, id: &str) -> SearchScope {
        SearchScope {
            id: id.into(),
            name: id.into(),
            path: path.to_string_lossy().into_owned(),
            filename_enabled: true,
            content_enabled: false,
            external_allowed: false,
        }
    }
    fn wait(index: &FileIndex, check: impl Fn() -> bool) {
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_secs(10) {
            if !index.status().unwrap().scanning && check() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("Index did not settle: {:?}", index.status().unwrap().count);
    }
    #[test]
    fn korean_search_filters_overlap_live_updates_and_scope_removal() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("하위");
        fs::create_dir(&sub).unwrap();
        let first = sub.join("공문서 계획.HWPX");
        fs::write(&first, "private content").unwrap();
        fs::write(sub.join("다른 계획.txt"), "").unwrap();
        let index = FileIndex::new();
        index.configure(vec![scope(dir.path(), "all"), scope(&sub, "sub")]);
        wait(&index, || index.status().unwrap().count == 4);
        let result = index
            .search(
                "계획 공문서 extension:hwpx edit-date:>2023",
                "",
                Some("sub"),
                20,
            )
            .unwrap();
        assert_eq!(result.total, 1);
        assert_eq!(result.results[0].name, "공문서 계획.HWPX");
        assert_eq!(index.search("계획", "txt", None, 20).unwrap().total, 1);
        assert_eq!(index.search("계획", "", Some("gone"), 20).unwrap().total, 0);
        assert!(index.search("edit-date:2024-02-30", "", None, 20).is_err());
        assert!(index.search("\"unfinished", "", None, 20).is_err());
        let moved = sub.join("새이름.hwpx");
        fs::rename(&first, &moved).unwrap();
        wait(&index, || {
            index.search("새이름", "", None, 20).unwrap().total == 1
        });
        assert_eq!(index.search("공문서", "", None, 20).unwrap().total, 0);
        assert!(index.resolve_result(first.to_str().unwrap()).is_err());
        fs::remove_file(&moved).unwrap();
        wait(&index, || {
            index.search("새이름", "", None, 20).unwrap().total == 0
        });
        index.configure(vec![]);
        assert_eq!(index.search("계획", "", None, 20).unwrap().total, 0);
    }
    #[test]
    fn decomposed_unicode_ranking_and_bounded_results() {
        let dir = tempfile::tempdir().unwrap();
        // APFS treats canonically equivalent filenames as the same entry.
        // Keep this a distinct file while still exercising a decomposed name.
        for name in [
            "가.txt",
            "가나.txt",
            "x가.txt",
            "가나다.txt",
            "가나다-decomposed.txt",
        ] {
            fs::write(dir.path().join(name), "").unwrap();
        }
        let index = FileIndex::new();
        index.configure(vec![scope(dir.path(), "all")]);
        wait(&index, || index.status().unwrap().count == 6);
        let result = index.search("가", "", None, 2).unwrap();
        assert_eq!(result.total, 5);
        assert_eq!(result.results.len(), 2);
        assert_eq!(result.results[0].name, "가.txt");
        assert_eq!(index.search("가나다", "", None, 20).unwrap().total, 2);
        assert_eq!(index.search("ㄱㄴㄷ", "", None, 20).unwrap().total, 2);
        assert!(!under("c:/docs-private/x", "c:/docs"));
    }
    #[test]
    fn initial_queries_preserve_filters_and_prioritize_literal_names() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["공문서.hwpx", "공문서.txt", "김민수.hwpx", "ㄱㅁㅅ.hwpx"] {
            fs::write(dir.path().join(name), "").unwrap();
        }
        let index = FileIndex::new();
        index.configure(vec![scope(dir.path(), "all")]);
        wait(&index, || index.status().unwrap().count == 5);
        let result = index.search("ㄱㅁㅅ ext:hwpx", "", None, 20).unwrap();
        assert_eq!(result.total, 3);
        assert_eq!(result.results[0].name, "ㄱㅁㅅ.hwpx");
        assert_eq!(
            index.search("ㄱ문서 ext:hwpx", "", None, 20).unwrap().total,
            1
        );
        assert_eq!(index.search("ㄱㅁㅅ", "txt", None, 20).unwrap().total, 1);
    }
    #[test]
    fn keyboard_fallback_runs_only_after_filtered_literal_results_are_empty() {
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "공문서.hwpx",
            "Prism-guide.txt",
            "쌀.md",
            "살.md",
            "rhdanstj-note.pdf",
        ] {
            fs::write(dir.path().join(name), "").unwrap();
        }
        let index = FileIndex::new();
        index.configure(vec![scope(dir.path(), "all")]);
        wait(&index, || index.status().unwrap().count == 6);
        let literal = index.search("rhdanstj", "", None, 20).unwrap();
        assert_eq!(literal.total, 1);
        assert!(!literal.keyboard_corrected);
        assert_eq!(literal.results[0].name, "rhdanstj-note.pdf");
        let corrected = index.search("rhdanstj ext:hwpx", "", None, 20).unwrap();
        assert_eq!(corrected.total, 1);
        assert!(corrected.keyboard_corrected);
        assert_eq!(corrected.results[0].name, "공문서.hwpx");
        assert_eq!(
            index
                .search_with_keyboard("rhdanstj", "hwpx", None, 20, false)
                .unwrap()
                .total,
            0
        );
        assert_eq!(
            index.search("ㅖㄱㅑ느", "", None, 20).unwrap().results[0].name,
            "Prism-guide.txt"
        );
        assert_eq!(
            index.search("Tkf", "md", None, 20).unwrap().results[0].name,
            "쌀.md"
        );
        assert_eq!(
            index.search("tkf", "md", None, 20).unwrap().results[0].name,
            "살.md"
        );
        let missing = index
            .search("rhdanstj", "hwpx", Some("missing"), 20)
            .unwrap();
        assert_eq!(missing.total, 0);
        assert!(!missing.keyboard_corrected);
    }
    #[test]
    fn snapshot_streams_every_match_with_the_same_filters_and_revision() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..505 {
            fs::write(dir.path().join(format!("공문서-{i:03}.md")), "").unwrap();
        }
        fs::create_dir(dir.path().join("공문서-folder")).unwrap();
        let index = FileIndex::new();
        index.configure(vec![scope(dir.path(), "all")]);
        wait(&index, || index.status().unwrap().count == 507);
        let response = index.search("rhdanstj", "md", Some("all"), 200).unwrap();
        assert_eq!(response.total, 505);
        assert_eq!(response.results.len(), 200);
        assert!(response.keyboard_corrected);
        let mut search = FilenameSearch {
            query: "rhdanstj".into(),
            extension: "md".into(),
            scope_id: Some("all".into()),
            index_revision: response.status.revision,
            settings_revision: 0,
        };
        let mut paths = Vec::new();
        assert!(!index
            .snapshot_files(&search, true, &AtomicBool::new(false), &mut |path| {
                paths.push(path.to_owned());
                Ok::<_, ()>(())
            })
            .ok()
            .unwrap());
        assert_eq!(paths.len(), 505);
        assert!(paths.iter().all(|path| path.ends_with(".md")));
        search.index_revision += 1;
        assert!(index
            .snapshot_files(
                &search,
                true,
                &AtomicBool::new(false),
                &mut |_| Ok::<_, ()>(())
            )
            .is_err());
    }
}
#[cfg(test)]
mod performance_tests {
    use super::*;
    fn raw(path: &str, modified: i64) -> Raw {
        let path = PathBuf::from(path);
        Raw {
            key: path_key(&path),
            name: path.file_name().unwrap().to_string_lossy().into_owned(),
            is_dir: false,
            size: 1,
            modified_ms: Some(modified),
            path,
        }
    }
    #[test]
    fn repeated_download_notifications_do_not_overflow_the_queue() {
        let mut queue = PendingPaths::default();
        for _ in 0..20_000 {
            queue.push([PathBuf::from("C:/models/model.part")]);
        }
        let (overflow, paths) = queue.take();
        assert!(
            !overflow,
            "one busy file must not trigger a whole-drive scan"
        );
        assert_eq!(paths, [PathBuf::from("C:/models/model.part")]);
        for n in 0..4097 {
            queue.push([PathBuf::from(format!("C:/files/{n}.txt"))]);
        }
        assert!(queue.take().0, "actual event loss must still reconcile");
        assert!(!queue.take().0);
    }
    #[test]
    fn overlapping_walks_are_coalesced_without_losing_sibling_prefixes() {
        let mut queue = PendingPaths::default();
        queue.push(
            [
                "C:/a/x",
                "C:/a",
                "C:/a-b",
                "C:/a-b/y",
                "C:/ab/z",
                "C:/a/x/deep",
            ]
            .map(PathBuf::from),
        );
        assert_eq!(
            queue.take().1,
            ["C:/a", "C:/a-b", "C:/ab/z"].map(PathBuf::from)
        );
        queue.push(["C:/", "C:/a", "D:/b"].map(PathBuf::from));
        assert_eq!(queue.take().1, ["C:/", "D:/b"].map(PathBuf::from));
    }
    #[test]
    fn parallel_walk_keeps_every_nested_file_and_cancels_revoked_scopes() {
        let dir = tempfile::tempdir().unwrap();
        for n in 0..32 {
            let folder = dir.path().join(format!("{n:02}/중첩"));
            fs::create_dir_all(&folder).unwrap();
            for file in 0..40 {
                fs::write(
                    folder.join(format!("문서-{file:02}.md")),
                    format!("{n}:{file}"),
                )
                .unwrap();
            }
        }
        let expected: BTreeMap<_, _> = WalkDir::new(dir.path())
            .into_iter()
            .map(|entry| {
                let entry = entry.unwrap();
                let metadata = entry.metadata().unwrap();
                (
                    path_key(entry.path()),
                    (
                        metadata.is_dir(),
                        if metadata.is_dir() { 0 } else { metadata.len() },
                    ),
                )
            })
            .collect();
        let index = FileIndex::new();
        let scope = || SearchScope {
            id: "all".into(),
            name: "all".into(),
            path: dir.path().to_string_lossy().into_owned(),
            filename_enabled: true,
            content_enabled: false,
            external_allowed: false,
        };
        index.configure(vec![scope()]);
        settle(&index, expected.len());
        let actual: BTreeMap<_, _> = index
            .catalog
            .read()
            .unwrap()
            .entries
            .iter()
            .map(|(key, entry)| (key.to_string(), (entry.is_dir, entry.size)))
            .collect();
        assert_eq!(actual, expected);
        index.refresh(vec![scope()]);
        index.configure(vec![]);
        settle(&index, 0);
        assert!(index.catalog.read().unwrap().entries.is_empty());
        index.configure(vec![scope()]);
        settle(&index, expected.len());
    }
    fn settle(index: &FileIndex, count: usize) {
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_secs(10) {
            let status = index.status().unwrap();
            if !status.scanning && status.count == count {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("Index did not settle at {count}");
    }
    #[test]
    fn entry_derives_name_extension_and_initials_from_one_string() {
        let entry = Entry::new(raw("C:/문서함/공문서 계획.HWPX", 1), 1);
        assert_eq!(entry.name(), "공문서 계획.hwpx");
        assert_eq!(entry.extension(), "hwpx");
        assert_eq!(entry.initials_name(), Some("ㄱㅁㅅ ㄱㅎ.hwpx"));
        assert_eq!(entry.result().name, "공문서 계획.HWPX");
        assert_eq!(Entry::new(raw("C:/a/.gitignore", 1), 1).extension(), "");
        assert_eq!(
            Entry::new(raw("C:/a/archive.tar.gz", 1), 1).extension(),
            "gz"
        );
        let ascii = Entry::new(raw("C:/a/plain.txt", 1), 1);
        assert!(ascii.initials_path.is_none() && ascii.keyboard_path.is_none());
    }
    #[test]
    fn subtree_ranges_exclude_sibling_prefixes() {
        let mut entries = BTreeMap::new();
        for path in [
            "C:/a",
            "C:/a/x.txt",
            "C:/a b/y.txt",
            "C:/a-b/z.txt",
            "C:/a/deep/w.txt",
            "C:/ab",
        ] {
            let entry = Entry::new(raw(path, 1), 1);
            entries.insert(entry.key.clone(), entry);
        }
        let keys: Vec<_> = subtree(&entries, &path_key(Path::new("C:/a")))
            .map(|entry| entry.result().name)
            .collect();
        assert_eq!(keys, ["a", "w.txt", "x.txt"]);
    }
    #[test]
    fn parallel_top_matches_equal_a_full_sort() {
        let entries: Vec<Entry> = (0..50_000)
            .map(|index| {
                Entry::new(
                    raw(
                        &format!("C:/r/{:05}-{}.txt", (index * 7919) % 50_000, index % 3),
                        1,
                    ),
                    1,
                )
            })
            .collect();
        let candidates: Vec<&Entry> = entries.iter().collect();
        let rank = |entry: &Entry| {
            entry
                .name()
                .ends_with("-1.txt")
                .then(|| entry.name().as_bytes()[2] % 4)
        };
        let (total, top) = top_matches(&candidates, 50, rank);
        let mut expected: Vec<Ranked> = candidates
            .iter()
            .filter_map(|&entry| rank(entry).map(|rank| Ranked { rank, entry }))
            .collect();
        assert_eq!(total, expected.len());
        expected.sort();
        expected.truncate(50);
        let names = |items: &[Ranked]| {
            items
                .iter()
                .map(|item| item.entry.key.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(names(&top), names(&expected));
    }
    #[test]
    fn reconcile_keeps_unchanged_entries_without_revision_churn_and_drops_stale_ones() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["one.txt", "two.txt"] {
            fs::write(dir.path().join(name), "").unwrap();
        }
        let index = FileIndex::new();
        index.configure(vec![SearchScope {
            id: "all".into(),
            name: "all".into(),
            path: dir.path().to_string_lossy().into_owned(),
            filename_enabled: true,
            content_enabled: false,
            external_allowed: false,
        }]);
        settle(&index, 3);
        let generation = index.generation.load(Ordering::SeqCst);
        let roots: Vec<PathBuf> = index
            .catalog
            .read()
            .unwrap()
            .roots
            .iter()
            .map(|root| root.path.clone())
            .collect();
        let before = index.status().unwrap().revision;
        let pointer = |index: &FileIndex| {
            let catalog = index.catalog.read().unwrap();
            catalog
                .entries
                .values()
                .map(|entry| entry.path.as_ptr() as usize)
                .collect::<Vec<_>>()
        };
        let original = pointer(&index);
        index.scan(generation, &roots, 1_000, true);
        assert_eq!(
            index.status().unwrap().revision,
            before,
            "unchanged tree must not trigger re-search"
        );
        assert_eq!(
            pointer(&index),
            original,
            "unchanged entries are not rebuilt"
        );
        // An entry whose file vanished without a watcher event is removed by the next sweep.
        {
            let mut catalog = index.catalog.write().unwrap();
            let ghost = Entry::new(raw(&roots[0].join("ghost.txt").to_string_lossy(), 1), 1_000);
            catalog.entries.insert(ghost.key.clone(), ghost);
        }
        index.scan(generation, &roots, 1_001, true);
        let status = index.status().unwrap();
        assert_eq!(status.count, 3);
        assert!(status.revision > before);
        assert_eq!(index.search("ghost", "", None, 10).unwrap().total, 0);
    }
    #[test]
    fn reconfiguring_keeps_unchanged_scopes_and_drops_removed_ones_immediately() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        fs::write(first.path().join("keep-me.txt"), "").unwrap();
        fs::write(second.path().join("drop-me.txt"), "").unwrap();
        let scope = |dir: &Path, id: &str| SearchScope {
            id: id.into(),
            name: id.into(),
            path: dir.to_string_lossy().into_owned(),
            filename_enabled: true,
            content_enabled: false,
            external_allowed: false,
        };
        let index = FileIndex::new();
        index.configure(vec![scope(first.path(), "a"), scope(second.path(), "b")]);
        settle(&index, 4);
        let pointer = |index: &FileIndex| {
            let catalog = index.catalog.read().unwrap();
            catalog
                .entries
                .values()
                .find(|entry| entry.name() == "keep-me.txt")
                .map(|entry| entry.path.as_ptr() as usize)
        };
        let original = pointer(&index);
        // Removing scope b: its results disappear synchronously, a's stay without a rebuild.
        index.configure(vec![scope(first.path(), "a")]);
        assert_eq!(index.search("drop-me", "", None, 10).unwrap().total, 0);
        assert_eq!(index.search("keep-me", "", None, 10).unwrap().total, 1);
        settle(&index, 2);
        assert_eq!(
            pointer(&index),
            original,
            "unchanged scope entries are reused"
        );
        assert_eq!(index.search("keep-me", "", Some("a"), 10).unwrap().total, 1);
        // Pointing scope a at another folder drops its old results immediately.
        index.configure(vec![scope(second.path(), "a")]);
        assert_eq!(index.search("keep-me", "", None, 10).unwrap().total, 0);
        settle(&index, 2);
        assert_eq!(index.search("drop-me", "", Some("a"), 10).unwrap().total, 1);
        index.configure(vec![]);
        assert_eq!(index.status().unwrap().count, 0);
    }

    #[test]
    fn renamed_folder_updates_children_incrementally() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("이전폴더");
        fs::create_dir(&folder).unwrap();
        for name in ["a.txt", "b.txt"] {
            fs::write(folder.join(name), "").unwrap();
        }
        fs::write(dir.path().join("outside.txt"), "").unwrap();
        let index = FileIndex::new();
        index.configure(vec![SearchScope {
            id: "all".into(),
            name: "all".into(),
            path: dir.path().to_string_lossy().into_owned(),
            filename_enabled: true,
            content_enabled: false,
            external_allowed: false,
        }]);
        settle(&index, 5);
        fs::rename(&folder, dir.path().join("새폴더")).unwrap();
        let start = std::time::Instant::now();
        while index.search("새폴더 txt", "", None, 10).unwrap().total != 2 {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "rename not applied"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        settle(&index, 5);
        assert_eq!(index.search("이전폴더", "", None, 10).unwrap().total, 0);
        assert_eq!(index.search("outside", "", None, 10).unwrap().total, 1);
        assert_eq!(index.search("ㅅㅍㄷ", "", None, 10).unwrap().total, 3);
    }
}

#[cfg(test)]
mod bench {
    use super::*;
    /// `cargo test --release bench_ -- --ignored --nocapture` — synthetic 100k-entry catalog.
    #[test]
    #[ignore]
    fn bench_search_and_incremental_update() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        for folder in 0..100 {
            let sub = root.join(format!("{folder:03}_공문서_folder"));
            fs::create_dir_all(&sub).unwrap();
            for file in 0..1000 {
                fs::write(sub.join(format!("report_{file:04}_계획.txt")), "").unwrap();
            }
        }
        let index = FileIndex::new();
        let start = std::time::Instant::now();
        index.configure(vec![SearchScope {
            id: "root".into(),
            name: "root".into(),
            path: root.to_string_lossy().into_owned(),
            filename_enabled: true,
            content_enabled: false,
            external_allowed: false,
        }]);
        while index.status().unwrap().scanning || index.status().unwrap().count < 100_101 {
            std::thread::sleep(Duration::from_millis(20));
        }
        println!("BENCH initial scan: {:?}", start.elapsed());
        for query in [
            "report_0999",
            "계획",
            "ㄱㅎ",
            "zzzz-nothing",
            "folder ext:txt edit-date:>2000",
        ] {
            let start = std::time::Instant::now();
            let runs = 10;
            let mut total = 0;
            for _ in 0..runs {
                total = index.search(query, "", None, 200).unwrap().total;
            }
            println!(
                "BENCH search {query:?}: {:?}/query, total {total}",
                start.elapsed() / runs
            );
        }
        let start = std::time::Instant::now();
        let runs = 10;
        for _ in 0..runs {
            index.search("report", "", Some("root"), 200).unwrap();
        }
        println!("BENCH scoped search: {:?}/query", start.elapsed() / runs);
        let target = root.join("050_공문서_folder").join("new-file.txt");
        let start = std::time::Instant::now();
        fs::write(&target, "").unwrap();
        while index.search("new-file", "", None, 1).unwrap().total == 0 {
            std::thread::sleep(Duration::from_millis(5));
        }
        println!("BENCH live add visible after: {:?}", start.elapsed());
    }
}
