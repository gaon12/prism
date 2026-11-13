use crate::filename_pattern::{initial_projection, FilenamePattern};
use crate::keyboard_pattern::{keyboard_projection, KeyboardPattern};
use crate::settings::SearchScope;
use chrono::{Datelike, Local, NaiveDate, TimeZone};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use std::{
    borrow::Cow,
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
#[path = "filename_compact.rs"]
mod compact;
#[path = "filename_entries.rs"]
mod entries;
#[path = "filename_service.rs"]
mod service;
use entries::{Entries, Scratch, View};

fn normalized(value: &str) -> String {
    value
        .nfc()
        .collect::<String>()
        .to_lowercase()
        .replace('\\', "/")
}
fn path_key(path: &Path) -> String {
    path_key_text(&path.to_string_lossy())
}

/// How many CPUs this process may actually run on, for sizing worker threads.
///
/// `std::thread::available_parallelism` counts every CPU of the machine on
/// Windows, even when the process is limited to fewer (an affinity mask or a
/// Job Object, as the low-end test profile and some managed PCs do). Eight
/// workers sharing one CPU then crowd out the window's own threads: on a
/// 1-CPU limit the first results appeared after 9-20 s instead of about 2 s.
/// So on Windows the count is capped by the process affinity mask.
fn usable_cpus() -> usize {
    let all = std::thread::available_parallelism().map_or(1, usize::from);
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessAffinityMask};
        let (mut process, mut system) = (0usize, 0usize);
        // SAFETY: the pseudo handle needs no closing; both pointers are valid locals.
        if unsafe { GetProcessAffinityMask(GetCurrentProcess(), &mut process, &mut system) } != 0 {
            return cpus_allowed(all, process);
        }
    }
    all
}

/// Run the calling indexing thread below normal priority (Windows).
///
/// On a single-CPU machine the filename collection and the window's own
/// threads take turns on that one CPU. At equal priority a fast service kept
/// the window from starting for 10-18 s. Below-normal priority lets the window
/// and search answer first; collection still uses every idle moment, so it is
/// not throttled when nothing else needs the CPU. Disk priority is unchanged.
fn background_thread() {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{
            GetCurrentThread, SetThreadPriority, SetThreadPriorityBoost,
            THREAD_PRIORITY_BELOW_NORMAL,
        };
        // SAFETY: the pseudo handle names the calling thread and needs no closing.
        // A failure only leaves the normal priority in place.
        unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL) };
        // Windows briefly raises a thread's priority each time its I/O completes.
        // The snapshot reader completes a pipe read for every batch, so with the
        // boost it ran at 9-11 most of the time: above the window's renderer (8),
        // which made below-normal useless on one CPU (window ready at 12-14 s).
        // SAFETY: as above; TRUE (1) disables the boost for this thread only.
        unsafe { SetThreadPriorityBoost(GetCurrentThread(), 1) };
    }
}

/// `all` CPUs limited to the CPUs set in an affinity `mask` (at least one).
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
fn cpus_allowed(all: usize, mask: usize) -> usize {
    match mask.count_ones() as usize {
        0 => all.max(1),
        allowed => allowed.min(all).max(1),
    }
}
fn path_key_text(value: &str) -> String {
    let value = value
        .strip_prefix(r"\\?\UNC\")
        .map(|tail| Cow::Owned(format!(r"\\{tail}")))
        .unwrap_or_else(|| Cow::Borrowed(value.strip_prefix(r"\\?\").unwrap_or(value)));
    #[cfg(windows)]
    if value.is_ascii() {
        // One allocation for the common whole-drive case, rather than copying
        // the path for prefix removal, lowercase, separators and trimming.
        let bytes = value
            .bytes()
            .map(|byte| {
                if byte == b'\\' {
                    b'/'
                } else {
                    byte.to_ascii_lowercase()
                }
            })
            .collect();
        let mut key = String::from_utf8(bytes).expect("ASCII path key");
        key.truncate(key.trim_end_matches('/').len());
        return key;
    }
    #[cfg(windows)]
    let value = value.to_lowercase();
    // Keep filesystem identity distinct from the NFC-normalized search text.
    let mut key = value.replace('\\', "/");
    key.truncate(key.trim_end_matches('/').len());
    key
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
/// The catalog's ordering key shares the identity allocation with this entry.
struct Entry {
    path: Box<str>,
    key: Arc<str>,
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
        // Windows identity keys have already been lowercased by path_key_text.
        // Avoid rescanning every ASCII byte on a multi-million-file drive.
        // Unix identity is case-sensitive, so its display/search split remains.
        let search =
            if ascii && (cfg!(windows) || !raw.key.bytes().any(|byte| byte.is_ascii_uppercase())) {
                Cow::Borrowed(raw.key.as_str())
            } else if ascii {
                Cow::Owned(raw.key.to_ascii_lowercase())
            } else {
                Cow::Owned(normalized(&raw.key))
            };
        // Initial projection keeps UTF-8 byte lengths (3-byte syllable → 3-byte jamo), so the
        // name offset is valid for both strings.
        let name_start = if !raw.has_name {
            search.len()
        } else {
            search.rfind('/').map_or(0, |index| index + 1)
        } as u32;
        Self {
            path: raw.path.into(),
            initials_path: (!ascii)
                .then(|| initial_projection(&search))
                .flatten()
                .map(Into::into),
            keyboard_path: (!ascii)
                .then(|| keyboard_projection(&search))
                .flatten()
                .map(Into::into),
            search: (search.as_ref() != raw.key).then(|| search.into_owned().into()),
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
    #[cfg(test)]
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
    path: String,
    key: String,
    has_name: bool,
    is_dir: bool,
    size: u64,
    modified_ms: Option<i64>,
}
/// What one walk job reads.
#[derive(Clone, Copy)]
enum Walk {
    /// A changed path and everything below it.
    Tree,
    /// The children of a folder whose own entry its parent's job recorded.
    Children,
    /// Only the path's own entry; the folder's children are unchanged.
    Entry,
}
struct WalkJobs {
    paths: VecDeque<(PathBuf, Walk)>,
    outstanding: usize,
}
struct WalkBatch {
    rows: Vec<Raw>,
    skipped: usize,
}
#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexStatus {
    pub scanning: bool,
    /// Remains true during live updates after the first complete sweep of this
    /// configured generation. A refresh resets it, even if old entries remain.
    pub initial_scan_complete: bool,
    pub count: usize,
    pub revision: u64,
    pub skipped: usize,
    pub warnings: Vec<String>,
    pub watching: bool,
    /// Available only in the user-requested diagnostic projection. Normal
    /// status/search polls keep their existing DTO and revision semantics.
    #[serde(skip)]
    pub(crate) last_service_snapshot: Option<ServiceSnapshotTiming>,
    /// Diagnostics only, counted since the last refresh: validated service
    /// snapshots, and full re-collections started because live-update events
    /// were lost (the OS buffer or the pending-path bound overflowed). Each such
    /// re-collection repeats a whole-drive snapshot, so it shows why a run
    /// used more time or CPU than one collection.
    #[serde(skip)]
    pub(crate) service_snapshots: u32,
    #[serde(skip)]
    pub(crate) overflow_reconciles: u32,
}
/// One successfully validated service snapshot, not an entire GUI scan. Fetch
/// overlaps private preparation and includes callback/backpressure wall time;
/// callback time is a subset of fetch and must never be added to it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ServiceSnapshotTiming {
    pub received_records: usize,
    pub first_batch_ms: Option<u64>,
    pub validated_fetch_ms: u64,
    pub receive_callback_ms: u64,
    pub finish_publish_ms: u64,
    pub total_ms: u64,
    pub initial_catalog: bool,
}
#[derive(Clone)]
struct Root {
    id: String,
    key: String,
    path: PathBuf,
    /// The scope path as configured, used to tell whether a reconfigured scope is unchanged.
    source: String,
    /// An unchanged unavailable scope keeps its known identity for cached
    /// filename filtering, but must not automatically enumerate its old target.
    available: bool,
}
#[derive(Default)]
struct Catalog {
    /// Compact storage: each directory once, each entry as a small record.
    entries: Entries,
    roots: Vec<Root>,
    status: IndexStatus,
    /// The very first scan's rows while its NTFS snapshot is still arriving.
    /// See `Provisional`.
    provisional: Option<Provisional>,
}

/// A first scan's catalog that searches may read before the snapshot ends.
///
/// The NTFS service sends a whole drive in a few seconds of frames, and a
/// snapshot is only trusted once its terminal frame and the open root are
/// validated. Rows are prepared in this separate catalog meanwhile. Searches
/// show it only while the real catalog is still empty and its generation is
/// current, so the user sees results seconds earlier. It never becomes the
/// real catalog unless the whole snapshot validates: on failure, cancellation
/// or a scope change it is simply detached, and nothing has to be removed
/// from (or told apart in) the real catalog.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
struct Provisional {
    generation: u64,
    entries: Arc<Mutex<Entries>>,
}

/// Whether a watch event changed only its path's own entry, so the folder
/// below it need not be walked again.
///
/// On Windows a "modified" notice for a folder means a child was added,
/// removed or renamed, or the folder's own times changed. Each such child is
/// also reported on its own, and lost notices raise a rescan, so the folder's
/// own entry is all that is left to read. Walking the whole folder for it
/// re-read large folders such as the user's temporary files (about 46,000
/// entries) many times a minute; on one CPU that kept collection busy for
/// seconds after every first catalog. Other platforms keep the recursive walk.
/// A real file or folder, not a link or another reparse point such as a
/// junction (the walker skips those, so only a full walk updates them).
fn plain_entry(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // FILE_ATTRIBUTE_REPARSE_POINT
        if metadata.file_attributes() & 0x400 != 0 {
            return false;
        }
    }
    !metadata.file_type().is_symlink()
}
fn entry_only(kind: &EventKind) -> bool {
    cfg!(windows) && matches!(kind, EventKind::Modify(notify::event::ModifyKind::Any))
}

/// Bound queued recursive subtrees, not notifications: one busy file or an
/// already queued parent covers repeated writes throughout that subtree.
#[derive(Default)]
struct PendingPaths {
    paths: BTreeMap<String, PathBuf>,
    /// Paths whose own entry changed while the folder below them did not (see
    /// [`entry_only`]). Only the entry itself is read again. A queued walk of
    /// the path or of a parent covers it, so it is dropped then.
    entries: BTreeMap<String, PathBuf>,
    overflow: bool,
}
impl PendingPaths {
    /// Whether a queued recursive walk of `key` itself or of a parent covers it.
    fn covers(&self, key: &str) -> bool {
        let mut ancestor = key;
        loop {
            if self.paths.contains_key(ancestor) {
                return true;
            }
            match ancestor.rsplit_once('/') {
                Some((parent, _)) => ancestor = parent,
                None => return false,
            }
        }
    }
    fn push(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        for path in paths {
            let key = path_key(&path);
            if self.covers(&key) {
                continue;
            }
            // This path will be scanned recursively. Replace its descendants
            // before testing the bound, even when the queue is already full.
            // The trailing separator keeps sibling prefixes independent.
            let prefix = format!("{key}/");
            let descendants: Vec<_> = self
                .paths
                .range::<str, _>((
                    std::ops::Bound::Included(prefix.as_str()),
                    std::ops::Bound::Unbounded,
                ))
                .take_while(|(candidate, _)| candidate.starts_with(&prefix))
                .map(|(key, _)| key.clone())
                .collect();
            for descendant in descendants {
                self.paths.remove(&descendant);
            }
            // Entry refreshes at or below this path are covered by its walk too.
            let entries: Vec<_> = self
                .entries
                .range::<str, _>((
                    std::ops::Bound::Included(prefix.as_str()),
                    std::ops::Bound::Unbounded,
                ))
                .take_while(|(candidate, _)| candidate.starts_with(&prefix))
                .map(|(key, _)| key.clone())
                .collect();
            for entry in entries {
                self.entries.remove(&entry);
            }
            self.entries.remove(&key);
            if self.paths.len() == 4096 {
                // Once a path was lost, later coalescing cannot establish that
                // it was covered. Keep overflow set until the consumer takes it.
                self.overflow = true;
                break;
            }
            self.paths.insert(key, path);
        }
    }
    /// Queue paths whose own entry changed but whose folder contents did not.
    /// Repeated notices for one folder take a single slot, and the same bound
    /// as walks applies: a lost refresh raises the reconcile instead.
    fn push_entries(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        for path in paths {
            let key = path_key(&path);
            if self.covers(&key) || self.entries.contains_key(&key) {
                continue;
            }
            if self.entries.len() == 4096 {
                self.overflow = true;
                break;
            }
            self.entries.insert(key, path);
        }
    }
    /// [`Self::take`] plus the queued entry refreshes.
    fn take_changes(&mut self) -> (bool, Vec<PathBuf>, Vec<PathBuf>) {
        let entries = std::mem::take(&mut self.entries).into_values().collect();
        let (overflow, paths) = self.take();
        (overflow, paths, entries)
    }
    fn take(&mut self) -> (bool, Vec<PathBuf>) {
        let overflow = std::mem::take(&mut self.overflow);
        (
            overflow,
            std::mem::take(&mut self.paths).into_values().collect(),
        )
    }
}

/// A failed directory read says nothing about whether its cached children
/// still exist. Keep this bounded like watch events; overflow preserves all
/// unseen entries for this sweep rather than guessing which failures were lost.
struct UncertainPaths {
    all: bool,
    roots: BTreeMap<String, PathBuf>,
    descendants: Vec<(String, String)>,
}
impl From<PendingPaths> for UncertainPaths {
    fn from(pending: PendingPaths) -> Self {
        let mut descendants: Vec<_> = if pending.overflow {
            vec![]
        } else {
            pending
                .paths
                .keys()
                .map(|key| (format!("{key}/"), format!("{key}0")))
                .collect()
        };
        // Root-key ordering differs from descendant-range ordering for sibling
        // prefixes such as /a-b and /a/. The queued subtrees never overlap.
        descendants.sort_unstable();
        Self {
            all: pending.overflow,
            roots: pending.paths,
            descendants,
        }
    }
}
impl UncertainPaths {
    fn contains(&self, key: &str) -> bool {
        if self.all || self.roots.contains_key(key) {
            return true;
        }
        let position = self
            .descendants
            .partition_point(|(low, _)| low.as_str() <= key);
        position > 0 && key < self.descendants[position - 1].1.as_str()
    }
}
fn note_unreadable(paths: &Mutex<PendingPaths>, path: &Path, kind: Option<std::io::ErrorKind>) {
    if kind != Some(std::io::ErrorKind::NotFound) {
        paths
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push([path.to_path_buf()]);
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
            .filter(|root| root.available)
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
enum RefreshKind {
    Configure,
    Refresh,
    Reset,
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
        std::thread::spawn(move || {
            background_thread();
            worker.run(rx)
        });
        this
    }
    pub fn configure(&self, scopes: Vec<SearchScope>) {
        self.request_refresh(scopes, RefreshKind::Configure);
    }
    pub fn refresh(&self, scopes: Vec<SearchScope>) {
        self.request_refresh(scopes, RefreshKind::Refresh);
    }
    pub fn reset(&self, scopes: Vec<SearchScope>) {
        self.request_refresh(scopes, RefreshKind::Reset);
    }
    fn request_refresh(&self, scopes: Vec<SearchScope>, kind: RefreshKind) {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        if let Ok(mut catalog) = self.catalog.write() {
            if self.generation.load(Ordering::SeqCst) != generation {
                return;
            }
            // An unfinished first scan belongs to the old generation.
            catalog.provisional = None;
            // Removed or disabled scopes must stop being searchable immediately. Entries under
            // scopes that are still configured unchanged stay visible while the rescan runs, so
            // adding a folder does not blank existing results.
            if matches!(kind, RefreshKind::Reset) {
                catalog.entries.clear();
                catalog.roots.clear();
            } else if matches!(kind, RefreshKind::Configure) {
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
            .map(|catalog| self.status_of(&catalog))
            .map_err(|_| "File index unavailable.".into())
    }
    /// The status a caller sees: a first scan still arriving reports the rows
    /// searches can already find.
    fn status_of(&self, catalog: &Catalog) -> IndexStatus {
        let mut status = catalog.status.clone();
        if let Some(entries) = self.provisional(catalog) {
            status.count = entries.len();
        }
        status
    }
    fn current(&self, generation: u64) -> bool {
        Arc::strong_count(&self.lifetime) > 1
            && self.generation.load(Ordering::SeqCst) == generation
    }
    /// The first scan's provisional rows, when searches should read them
    /// instead of the (still empty) real catalog. Lock order is always the
    /// catalog first, then the provisional rows.
    fn provisional<'a>(&self, catalog: &'a Catalog) -> Option<std::sync::MutexGuard<'a, Entries>> {
        catalog
            .provisional
            .as_ref()
            .filter(|provisional| {
                catalog.entries.is_empty() && self.current(provisional.generation)
            })
            .map(|provisional| {
                provisional
                    .entries
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
            })
    }
    fn resolve_roots(&self, scopes: &[SearchScope]) -> (Vec<Root>, Vec<String>) {
        let previous = self
            .catalog
            .read()
            .map(|catalog| catalog.roots.clone())
            .unwrap_or_default();
        let mut roots = vec![];
        let mut warnings = vec![];
        for scope in scopes.iter().filter(|scope| scope.filename_enabled) {
            match fs::canonicalize(&scope.path) {
                Ok(path) if path.is_dir() => roots.push(Root {
                    id: scope.id.clone(),
                    key: path_key(&path),
                    path,
                    source: scope.path.clone(),
                    available: true,
                }),
                _ => {
                    warnings.push(format!("Folder unavailable: {}", scope.path));
                    if let Some(root) = previous
                        .iter()
                        .find(|root| root.id == scope.id && root.source == scope.path)
                    {
                        let mut root = root.clone();
                        root.available = false;
                        roots.push(root);
                    }
                }
            }
        }
        (roots, warnings)
    }
    fn note_walk_failure(
        &self,
        generation: u64,
        unreadable: &Mutex<PendingPaths>,
        path: &Path,
        kind: Option<std::io::ErrorKind>,
    ) {
        let key = path_key(path);
        let configured_root = self.catalog.write().is_ok_and(|mut catalog| {
            if !self.current(generation) {
                return false;
            }
            let sources: Vec<_> = catalog
                .roots
                .iter_mut()
                .filter(|root| root.key == key)
                .map(|root| {
                    root.available = false;
                    root.source.clone()
                })
                .collect();
            if sources.is_empty() {
                return false;
            }
            catalog.status.watching = false;
            for source in sources {
                let warning = format!("Folder unavailable: {source}");
                if catalog.status.warnings.len() < 5 && !catalog.status.warnings.contains(&warning)
                {
                    catalog.status.warnings.push(warning);
                }
            }
            true
        });
        // A missing configured root is an unavailable scope. A missing child
        // is still a real deletion; do not extend cached identities to new paths.
        note_unreadable(unreadable, path, if configured_root { None } else { kind });
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
                    let (roots, mut warnings) = self.resolve_roots(&scopes);
                    if !self.current(generation) {
                        continue;
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
                                    if entry_only(&event.kind) {
                                        paths.push_entries(event.paths);
                                    } else {
                                        paths.push(event.paths);
                                    }
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
            let (queue_overflow, paths, entries) = match pending.lock() {
                Ok(mut paths) => paths.take_changes(),
                Err(_) => continue,
            };
            let overflowed = overflow.swap(false, Ordering::SeqCst) || queue_overflow;
            if overflowed {
                if let Ok(mut catalog) = self.catalog.write() {
                    catalog.status.overflow_reconciles += 1;
                }
            }
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
            if paths.is_empty() && entries.is_empty() {
                continue;
            }
            let roots = match self.catalog.read() {
                Ok(catalog) => catalog.roots.clone(),
                Err(_) => continue,
            };
            let available = |path: &PathBuf| {
                roots
                    .iter()
                    .any(|root| root.available && under(&path_key(path), &root.key))
            };
            let mut unique: Vec<_> = paths.into_iter().filter(available).collect();
            let mut entries: Vec<_> = entries.into_iter().filter(available).collect();
            // An entry refresh fits only a plain file or folder. A path that no
            // longer exists went away with everything below it, and a folder that
            // became a link or junction is no longer walked at all: walk such a
            // path like a removal so its cached entry and subtree are dropped.
            entries.retain(|path| {
                let plain = fs::symlink_metadata(path).is_ok_and(|metadata| plain_entry(&metadata));
                if !plain {
                    unique.push(path.clone());
                }
                plain
            });
            if !unique.is_empty() || !entries.is_empty() {
                // Re-walk only the changed subtrees under a fresh sweep id, then drop entries in
                // those subtrees that the walk no longer saw (deleted or renamed away).
                sweep += 1;
                self.scan_changes(generation, &unique, &entries, sweep);
            }
        }
    }
    /// Walks `paths`. A reconcile sweep (`reconcile`) also removes entries the walk did not see.
    fn scan(&self, generation: u64, paths: &[PathBuf], sweep: u64, reconcile: bool) {
        self.scan_paths(generation, paths, &[], sweep, reconcile);
    }
    /// A live update: walks each changed path in `paths` with everything below
    /// it, reads only the own entry of each path in `entries`, then drops
    /// entries under `paths` that the walk no longer saw.
    fn scan_changes(&self, generation: u64, paths: &[PathBuf], entries: &[PathBuf], sweep: u64) {
        self.scan_paths(generation, paths, entries, sweep, false);
    }
    fn scan_paths(
        &self,
        generation: u64,
        paths: &[PathBuf],
        entries: &[PathBuf],
        sweep: u64,
        reconcile: bool,
    ) {
        if !self.current(generation) {
            return;
        }
        if let Ok(mut catalog) = self.catalog.write() {
            if !self.current(generation) {
                return;
            }
            catalog.status.scanning = true;
            if reconcile {
                catalog.status.skipped = 0;
            }
        }
        // A single recursive walk serializes directory reads, Unicode work and
        // catalog insertion. Disjoint directory jobs overlap reads while each
        // worker keeps only a small publication batch. Nested input roots have
        // already been coalesced, so no two jobs enumerate the same directory.
        // Only complete drive-root reconciliations use the NTFS service.
        // Watch-event subtrees retain the bounded ordinary-user walker.
        let walk_paths: Vec<_> = paths
            .iter()
            .filter(|path| !reconcile || !self.try_service_scan(generation, path, sweep))
            .cloned()
            .collect();
        let jobs = Mutex::new(WalkJobs {
            paths: walk_paths
                .iter()
                .cloned()
                .map(|path| (path, Walk::Tree))
                .chain(entries.iter().cloned().map(|path| (path, Walk::Entry)))
                .collect(),
            outstanding: walk_paths.len() + entries.len(),
        });
        let wake = Condvar::new();
        let unreadable = Mutex::new(PendingPaths::default());
        let threads = usable_cpus().clamp(1, 8);
        std::thread::scope(|scope| {
            for _ in 0..threads {
                let jobs = &jobs;
                let wake = &wake;
                let unreadable = &unreadable;
                scope.spawn(move || {
                    background_thread();
                    let mut batch = WalkBatch {
                        rows: Vec::with_capacity(512),
                        skipped: 0,
                    };
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
                        let Some((path, walk)) = work else {
                            break;
                        };
                        let children = self
                            .walk_directory(generation, &path, walk, sweep, &mut batch, unreadable);
                        let mut jobs = jobs.lock().unwrap_or_else(|error| error.into_inner());
                        jobs.outstanding += children.len();
                        jobs.outstanding -= 1;
                        jobs.paths
                            .extend(children.into_iter().map(|path| (path, Walk::Children)));
                        wake.notify_all();
                    }
                    self.publish(generation, sweep, &mut batch.rows, batch.skipped, false);
                });
            }
        });
        let uncertain = UncertainPaths::from(unreadable.into_inner().unwrap_or_else(|error| {
            let mut paths = error.into_inner();
            paths.overflow = true;
            paths
        }));
        if let Ok(mut catalog) = self.catalog.write() {
            if !self.current(generation) {
                return;
            }
            let before = catalog.entries.len();
            if reconcile {
                let unavailable: Vec<_> = catalog
                    .roots
                    .iter()
                    .filter(|root| !root.available)
                    .map(|root| root.key.clone())
                    .collect();
                let visited: Vec<_> = paths.iter().map(|path| path_key(path)).collect();
                // Seen entries are kept without rebuilding their keys.
                catalog.entries.retain_seen_or(sweep, |key, _| {
                    uncertain.contains(key)
                        // A still-configured offline parent must not hide a
                        // confirmed deletion in an available, visited child scope.
                        || (unavailable.iter().any(|root| under(key, root))
                            && !visited.iter().any(|root| under(key, root)))
                });
                catalog.status.initial_scan_complete = true;
            } else {
                // Keep watch updates limited to their own subtrees, without
                // re-scanning the whole catalog or erasing failed subtrees.
                let mut key = String::new();
                let stale: Vec<String> = paths
                    .iter()
                    .flat_map(|path| {
                        let root = path_key(path);
                        catalog
                            .entries
                            .subtree(&root)
                            // Rewalked entries are seen: skip them before
                            // building a key (most of a refreshed subtree).
                            .filter(|entry| !entry.is_seen(sweep))
                            .filter_map(|entry| {
                                let entry_key = entry.write_key(&mut key);
                                (!uncertain.contains(entry_key)).then(|| entry_key.to_string())
                            })
                            .collect::<Vec<_>>()
                    })
                    .collect();
                for key in stale {
                    catalog.entries.remove(&key);
                }
            }
            if catalog.entries.len() != before {
                catalog.status.revision += 1;
            }
            catalog.status.count = catalog.entries.len();
            catalog.status.scanning = false;
        }
    }
    fn walk_directory(
        &self,
        generation: u64,
        path: &Path,
        walk: Walk,
        sweep: u64,
        batch: &mut WalkBatch,
        unreadable: &Mutex<PendingPaths>,
    ) -> Vec<PathBuf> {
        let mut children = vec![];
        // Depth one makes directory ownership explicit. Keep WalkDir's cached
        // Windows metadata and the existing symlink/junction safeguards. An
        // entry refresh reads the path's own entry alone (depth zero).
        let walker = WalkDir::new(path)
            .max_depth(if matches!(walk, Walk::Entry) { 0 } else { 1 })
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
                    if entry.depth() == 0 && matches!(walk, Walk::Children) {
                        continue;
                    }
                    if !entry.file_type().is_dir() && !entry.file_type().is_file() {
                        continue;
                    }
                    let metadata = match entry.metadata() {
                        Ok(metadata) => metadata,
                        Err(error) => {
                            batch.skipped += 1;
                            self.note_walk_failure(
                                generation,
                                unreadable,
                                entry.path(),
                                error.io_error().map(std::io::Error::kind),
                            );
                            continue;
                        }
                    };
                    if entry.depth() > 0 && metadata.is_dir() {
                        children.push(entry.path().to_owned());
                    }
                    batch.rows.push(Raw {
                        key: path_key(entry.path()),
                        has_name: !entry.file_name().is_empty(),
                        is_dir: metadata.is_dir(),
                        size: if metadata.is_dir() { 0 } else { metadata.len() },
                        modified_ms: metadata
                            .modified()
                            .ok()
                            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                            .map(|duration| duration.as_millis() as i64),
                        path: display_path(entry.path()),
                    });
                    if batch.rows.len() >= 512 {
                        self.publish(generation, sweep, &mut batch.rows, batch.skipped, false);
                        batch.skipped = 0;
                    }
                }
                Err(error) => {
                    batch.skipped += 1;
                    self.note_walk_failure(
                        generation,
                        unreadable,
                        error.path().unwrap_or(path),
                        error.io_error().map(std::io::Error::kind),
                    );
                }
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
            Ok(catalog) => {
                let mut buffer = String::new();
                batch
                    .drain(..)
                    .map(|raw| {
                        let unchanged = catalog
                            .entries
                            .get(raw.key.as_str())
                            .is_some_and(|entry| entries::unchanged(entry, &raw, &mut buffer));
                        (raw, unchanged)
                    })
                    .collect()
            }
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
                        if let Some(id) = catalog.entries.get(key.as_str()).map(|entry| entry.id())
                        {
                            catalog.entries.set_seen(id, sweep);
                        }
                    }
                    Update::Replace(entry) => {
                        catalog.entries.insert(&entry);
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
        let provisional = self.provisional(&catalog);
        let entries = provisional.as_deref().unwrap_or(&catalog.entries);
        let limit = limit.clamp(1, 500);
        // A selected scope only scans its own directories' entries.
        let candidates = match scope_id {
            None => Candidates::All(entries.slots()),
            Some(id) => match catalog.roots.iter().find(|root| root.id == id) {
                Some(root) => {
                    Candidates::Ids(entries.subtree(&root.key).map(|entry| entry.id()).collect())
                }
                None => Candidates::Ids(vec![]),
            },
        };
        let (mut total, mut ranked) = top_matches(entries, &candidates, limit, false, |entry| {
            query.matches(entry).then(|| query.rank(entry))
        });
        let mut keyboard_corrected = false;
        if total == 0 && allow_keyboard && query.keyboard_terms.iter().any(Option::is_some) {
            (total, ranked) = top_matches(entries, &candidates, limit, true, |entry| {
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
                .filter_map(|ranked| entries.entry(ranked.id).map(entries::result))
                .collect(),
            total,
            status: {
                let mut status = catalog.status.clone();
                if provisional.is_some() {
                    status.count = entries.len();
                }
                status
            },
        })
    }
    pub fn resolve_result(&self, path: &str) -> Result<PathBuf, String> {
        let canonical =
            fs::canonicalize(path).map_err(|_| "This file no longer exists or is unavailable.")?;
        let key = path_key(&canonical);
        let catalog = self.catalog.read().map_err(|_| "File index unavailable.")?;
        // A row shown from the first scan's provisional catalog opens too.
        let known = catalog.entries.contains_key(key.as_str())
            || self
                .provisional(&catalog)
                .is_some_and(|entries| entries.contains_key(key.as_str()));
        if key != path_key(Path::new(path))
            || !known
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
        // Entries in the selected scope (none for an unknown scope id).
        let scoped = || -> Box<dyn Iterator<Item = compact::EntryRef<'_>> + '_> {
            match (&search.scope_id, root) {
                (None, _) => Box::new(catalog.entries.refs()),
                (Some(_), Some(root)) => Box::new(catalog.entries.subtree(&root.key)),
                (Some(_), None) => Box::new(std::iter::empty()),
            }
        };
        let mut scratch = Scratch::default();
        let literal =
            scoped().any(|entry| query.matches(&entries::view(entry, &mut scratch, false)));
        let corrected =
            !literal && allow_keyboard && query.keyboard_terms.iter().any(Option::is_some);
        let mut path = String::new();
        for entry in scoped() {
            if cancel.load(Ordering::Acquire) {
                return Err(SnapshotError::Index("Search cancelled.".into()));
            }
            if entry.is_dir() {
                continue;
            }
            let view = entries::view(entry, &mut scratch, corrected);
            let matched = if corrected {
                query.matches_keyboard(&view)
            } else {
                query.matches(&view)
            };
            if matched {
                visit(entry.write_path(&mut path)).map_err(SnapshotError::Visitor)?;
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
/// One ranked match. Name and key are copied only for matches that can
/// enter the top list, so ties order exactly as before: (rank, name, key).
struct Ranked {
    rank: u8,
    name: Box<str>,
    key: Box<str>,
    id: u32,
}
impl PartialEq for Ranked {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == CmpOrdering::Equal
    }
}
impl Eq for Ranked {}
impl PartialOrd for Ranked {
    fn partial_cmp(&self, other: &Self) -> Option<CmpOrdering> {
        Some(self.cmp(other))
    }
}
impl Ord for Ranked {
    fn cmp(&self, other: &Self) -> CmpOrdering {
        (self.rank, &self.name, &self.key).cmp(&(other.rank, &other.name, &other.key))
    }
}
/// The entries a query scans: every slot, or a scope's collected ids.
enum Candidates {
    All(usize),
    Ids(Vec<u32>),
}
impl Candidates {
    fn len(&self) -> usize {
        match self {
            Self::All(slots) => *slots,
            Self::Ids(ids) => ids.len(),
        }
    }
    fn id(&self, position: usize) -> u32 {
        match self {
            Self::All(_) => position as u32,
            Self::Ids(ids) => ids[position],
        }
    }
}
/// Counts matches and keeps the best `limit` by rank. Large candidate sets are split across
/// threads (the per-entry substring tests dominate search time). `keyboard` also rebuilds
/// the keyboard projection, which only the layout-correction pass reads.
fn top_matches(
    entries: &Entries,
    candidates: &Candidates,
    limit: usize,
    keyboard: bool,
    rank: impl Fn(&View<'_>) -> Option<u8> + Sync,
) -> (usize, Vec<Ranked>) {
    let scan = |range: std::ops::Range<usize>| {
        let mut heap: BinaryHeap<Ranked> = BinaryHeap::with_capacity(limit + 1);
        let mut total = 0;
        let mut scratch = Scratch::default();
        let mut key = String::new();
        for position in range {
            let Some(entry) = entries.entry(candidates.id(position)) else {
                continue;
            };
            let view = entries::view(entry, &mut scratch, keyboard);
            let Some(rank) = rank(&view) else {
                continue;
            };
            total += 1;
            let name = view.name();
            if heap.len() >= limit {
                // Skip without copying when this match cannot beat the worst kept one.
                let worst = heap.peek().expect("a full heap");
                match (rank, name).cmp(&(worst.rank, &*worst.name)) {
                    CmpOrdering::Greater => continue,
                    CmpOrdering::Equal if entry.write_key(&mut key) >= &*worst.key => continue,
                    _ => {}
                }
            }
            heap.push(Ranked {
                rank,
                name: name.into(),
                key: entry.write_key(&mut key).into(),
                id: entry.id(),
            });
            if heap.len() > limit {
                heap.pop();
            }
        }
        (total, heap)
    };
    let threads = usable_cpus().min(8);
    let count = candidates.len();
    let (total, heap) = if count < 20_000 || threads < 2 {
        scan(0..count)
    } else {
        let size = count.div_ceil(threads);
        std::thread::scope(|scope| {
            let workers: Vec<_> = (0..count)
                .step_by(size)
                .map(|start| {
                    let scan = &scan;
                    scope.spawn(move || scan(start..(start + size).min(count)))
                })
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
    fn matches(&self, entry: &View<'_>) -> bool {
        self.terms
            .iter()
            .all(|term| term.matches(entry.searchable(), entry.initials_path))
            && self.filters_match(entry)
    }
    fn matches_keyboard(&self, entry: &View<'_>) -> bool {
        self.filters_match(entry)
            && self
                .terms
                .iter()
                .zip(&self.keyboard_terms)
                .all(|(term, keyboard)| {
                    term.matches(entry.searchable(), entry.initials_path)
                        || keyboard.as_ref().is_some_and(|pattern| {
                            pattern.matches(entry.searchable(), entry.keyboard_path)
                        })
                })
    }
    fn keyboard_name_matches(&self, entry: &View<'_>) -> bool {
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
    fn filters_match(&self, entry: &View<'_>) -> bool {
        self.extensions
            .iter()
            .all(|set| !entry.is_dir && set.contains(entry.extension()))
            && self.dates.iter().all(|(start, end)| {
                entry
                    .modified_ms
                    .is_some_and(|time| time >= *start && time < *end)
            })
    }
    fn rank(&self, entry: &View<'_>) -> u8 {
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
#[path = "filename_scan_tests.rs"]
mod scan_tests;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn worker_threads_follow_the_cpus_the_process_may_use() {
        // A 1-CPU limit on a 16-CPU machine gives one worker, not sixteen.
        assert_eq!(cpus_allowed(16, 0b1), 1);
        assert_eq!(cpus_allowed(16, 0b1010), 2);
        // A mask never raises the machine's count, and an empty one is ignored.
        assert_eq!(cpus_allowed(4, usize::MAX), 4);
        assert_eq!(cpus_allowed(4, 0), 4);
        assert_eq!(cpus_allowed(0, 0), 1);
        assert!(usable_cpus() >= 1);
    }
    #[test]
    #[cfg(windows)]
    fn an_indexing_thread_runs_below_normal() {
        use windows_sys::Win32::System::Threading::{
            GetCurrentThread, GetThreadPriority, GetThreadPriorityBoost,
            THREAD_PRIORITY_BELOW_NORMAL,
        };
        std::thread::spawn(|| {
            background_thread();
            // SAFETY: the pseudo handle names this test thread; `disabled` is a valid local.
            let priority = unsafe { GetThreadPriority(GetCurrentThread()) };
            assert_eq!(priority, THREAD_PRIORITY_BELOW_NORMAL);
            let mut disabled = 0;
            assert_ne!(
                unsafe { GetThreadPriorityBoost(GetCurrentThread(), &mut disabled) },
                0
            );
            assert_eq!(disabled, 1, "I/O completions must not lift the thread");
        })
        .join()
        .unwrap();
    }
    #[test]
    fn explicit_reset_clears_catalog_and_prevents_old_scan_publication() {
        let (tx, _rx) = mpsc::channel();
        let index = FileIndex {
            catalog: Arc::new(RwLock::new(Catalog::default())),
            generation: Arc::new(AtomicU64::new(1)),
            tx,
            lifetime: Arc::new(()),
        };
        let _owner = index.clone();
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("reset-source.md");
        fs::write(&source, "original document stays").unwrap();
        index.scan(1, &[dir.path().into()], 1, true);
        assert_eq!(index.status().unwrap().count, 2);
        index.reset(vec![scope(dir.path(), "files")]);
        assert_eq!(index.status().unwrap().count, 0);
        assert!(!index.status().unwrap().initial_scan_complete);
        index.scan(1, &[dir.path().into()], 2, true);
        assert_eq!(
            index.status().unwrap().count,
            0,
            "old generation cannot refill reset catalog"
        );
        index.scan(2, &[dir.path().into()], 3, true);
        assert_eq!(index.status().unwrap().count, 2);
        assert_eq!(
            fs::read_to_string(source).unwrap(),
            "original document stays"
        );
    }
    #[test]
    fn initial_completion_survives_live_updates_but_not_refresh_or_revocation() {
        // No background worker: drive the same real scan/publication methods
        // deterministically so a short idle interval cannot hide the transition.
        let (tx, _rx) = mpsc::channel();
        let index = FileIndex {
            catalog: Arc::new(RwLock::new(Catalog::default())),
            generation: Arc::new(AtomicU64::new(1)),
            tx,
            lifetime: Arc::new(()),
        };
        let _owner = index.clone();
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("first.md"), "").unwrap();
        assert!(!index.status().unwrap().initial_scan_complete);
        index.scan(1, &[dir.path().into()], 1, true);
        assert!(index.status().unwrap().initial_scan_complete);
        assert_eq!(index.status().unwrap().count, 2);
        index.publish(1, 2, &mut vec![], 0, false);
        let updating = index.status().unwrap();
        assert!(updating.scanning && updating.initial_scan_complete);
        assert_eq!(
            serde_json::to_value(updating).unwrap()["initialScanComplete"],
            true
        );
        index.scan(1, &[dir.path().into()], 2, false);
        assert!(index.status().unwrap().initial_scan_complete);
        index.scan(1, &[dir.path().into()], 3, true);
        assert!(index.status().unwrap().initial_scan_complete);
        index.refresh(vec![scope(dir.path(), "files")]);
        let refreshed = index.status().unwrap();
        assert!(refreshed.scanning && !refreshed.initial_scan_complete);
        index.scan(1, &[dir.path().into()], 4, true);
        assert!(!index.status().unwrap().initial_scan_complete);
        index.scan(2, &[dir.path().into()], 5, true);
        assert!(index.status().unwrap().initial_scan_complete);
        index.configure(vec![]);
        assert!(!index.status().unwrap().initial_scan_complete);
    }
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
            has_name: path.file_name().is_some_and(|name| !name.is_empty()),
            is_dir: false,
            size: 1,
            modified_ms: Some(modified),
            path: display_path(&path),
        }
    }
    #[test]
    fn a_seen_entry_is_kept_without_asking_for_its_key() {
        let mut entries = Entries::default();
        for (path, seen) in [
            ("C:/a/seen-1.md", 7),
            ("C:/a/b/seen-2.md", 7),
            ("C:/a/old-kept.md", 6),
            ("C:/a/old-dropped.md", 6),
        ] {
            entries.insert(&Entry::new(raw(path, 1), seen));
        }
        let mut asked = Vec::new();
        entries.retain_seen_or(7, |key, _| {
            asked.push(key.to_string());
            key.ends_with("old-kept.md")
        });
        asked.sort();
        assert_eq!(
            asked,
            [
                path_key_text("C:/a/old-dropped.md"),
                path_key_text("C:/a/old-kept.md")
            ],
            "only entries the sweep did not see are checked by key"
        );
        let mut left: Vec<_> = entries
            .sorted()
            .into_iter()
            .map(|e| e.key.to_string())
            .collect();
        left.sort();
        let mut expected =
            ["C:/a/b/seen-2.md", "C:/a/old-kept.md", "C:/a/seen-1.md"].map(path_key_text);
        expected.sort();
        assert_eq!(left, expected);
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
    fn a_queued_subtree_covers_bulk_changes_without_requesting_a_full_reconcile() {
        let mut queue = PendingPaths::default();
        queue.push([PathBuf::from("C:/downloads/패키지")]);
        queue.push(
            (0..20_000)
                .map(|n| PathBuf::from(format!("C:/downloads/패키지/part-{n}/document.txt"))),
        );
        let (overflow, paths) = queue.take();
        assert!(
            !overflow,
            "the queued recursive subtree covers every change"
        );
        assert_eq!(paths, [PathBuf::from("C:/downloads/패키지")]);

        // A parent arriving at the limit must replace covered children before
        // capacity is checked. Similar sibling prefixes remain independent.
        queue.push((0..4096).map(|n| PathBuf::from(format!("C:/downloads/part-{n}"))));
        queue.push(["C:/downloads", "C:/downloads-old/keep.txt"].map(PathBuf::from));
        let (overflow, paths) = queue.take();
        assert!(!overflow);
        assert_eq!(
            paths,
            ["C:/downloads", "C:/downloads-old/keep.txt"].map(PathBuf::from)
        );
    }
    #[test]
    fn coalescing_after_actual_event_loss_keeps_the_reconcile_required() {
        let mut queue = PendingPaths::default();
        queue.push((0..4096).map(|n| PathBuf::from(format!("C:/downloads/part-{n}"))));
        queue.push([PathBuf::from("D:/lost-outside-the-subtree.txt")]);
        queue.push([PathBuf::from("C:/downloads")]);
        let (overflow, paths) = queue.take();
        assert!(
            overflow,
            "a later parent cannot recover an unrelated lost event"
        );
        assert_eq!(paths, [PathBuf::from("C:/downloads")]);
        assert!(!queue.take().0, "only consuming the overflow clears it");
    }
    #[test]
    fn entry_refreshes_take_one_slot_each_and_yield_to_queued_walks() {
        let mut queue = PendingPaths::default();
        queue.push_entries(["C:/temp", "C:/a/b.txt", "C:/a-b"].map(PathBuf::from));
        // A walk of C:/a covers the refresh below it but not the sibling C:/a-b.
        queue.push([PathBuf::from("C:/a")]);
        queue.push_entries(["C:/a/c.txt", "C:/a", "C:/temp"].map(PathBuf::from));
        let (overflow, paths, entries) = queue.take_changes();
        assert!(!overflow);
        assert_eq!(paths, [PathBuf::from("C:/a")]);
        assert_eq!(entries, ["C:/a-b", "C:/temp"].map(PathBuf::from));
        for _ in 0..20_000 {
            queue.push_entries([PathBuf::from("C:/temp")]);
        }
        assert_eq!(
            queue.take_changes(),
            (false, vec![], vec![PathBuf::from("C:/temp")]),
            "a busy folder must not trigger a whole-drive scan"
        );
        // A folder's own refresh and the walk of a child created in it keep
        // each other, in either order: neither covers the other.
        queue.push_entries([PathBuf::from("C:/temp")]);
        queue.push([PathBuf::from("C:/temp/new")]);
        assert_eq!(
            queue.take_changes(),
            (
                false,
                vec![PathBuf::from("C:/temp/new")],
                vec![PathBuf::from("C:/temp")]
            )
        );
        queue.push([PathBuf::from("C:/temp/new")]);
        queue.push_entries([PathBuf::from("C:/temp")]);
        assert_eq!(
            queue.take_changes(),
            (
                false,
                vec![PathBuf::from("C:/temp/new")],
                vec![PathBuf::from("C:/temp")]
            )
        );
        queue.push_entries((0..4097).map(|n| PathBuf::from(format!("C:/files/{n}.txt"))));
        assert!(
            queue.take_changes().0,
            "lost refreshes must still reconcile"
        );
        assert_eq!(queue.take_changes(), (false, vec![], vec![]));
    }
    #[test]
    fn only_plain_files_and_folders_take_an_entry_refresh() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("file.md"), "").unwrap();
        for path in [dir.path().to_owned(), dir.path().join("file.md")] {
            assert!(plain_entry(&fs::symlink_metadata(path).unwrap()));
        }
        #[cfg(windows)]
        {
            // A junction needs no privilege, unlike a symbolic link.
            let target = dir.path().join("target");
            let junction = dir.path().join("junction");
            fs::create_dir(&target).unwrap();
            let made = std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(&junction)
                .arg(&target)
                .output()
                .unwrap();
            assert!(made.status.success());
            assert!(!plain_entry(&fs::symlink_metadata(&junction).unwrap()));
        }
    }
    #[test]
    fn only_a_windows_modified_notice_skips_the_folder_walk() {
        use notify::event::{CreateKind, ModifyKind, RemoveKind, RenameMode};
        let modified = EventKind::Modify(ModifyKind::Any);
        assert_eq!(entry_only(&modified), cfg!(windows));
        for kind in [
            EventKind::Create(CreateKind::Any),
            EventKind::Remove(RemoveKind::Any),
            EventKind::Modify(ModifyKind::Name(RenameMode::To)),
            EventKind::Modify(ModifyKind::Name(RenameMode::From)),
            EventKind::Any,
        ] {
            assert!(!entry_only(&kind), "{kind:?} must walk the changed path");
        }
    }
    #[test]
    fn an_entry_refresh_reads_the_folder_alone_and_keeps_its_contents() {
        let (tx, _rx) = mpsc::channel();
        let index = FileIndex {
            catalog: Arc::new(RwLock::new(Catalog::default())),
            generation: Arc::new(AtomicU64::new(1)),
            tx,
            lifetime: Arc::new(()),
        };
        let _owner = index.clone();
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("temp");
        fs::create_dir_all(folder.join("deep")).unwrap();
        fs::write(folder.join("deep/kept.md"), "").unwrap();
        index.scan(1, &[dir.path().into()], 1, true);
        assert_eq!(index.status().unwrap().count, 4);
        // A file that arrives without its own notice stays unread: the refresh
        // reads the folder entry only. Everything below the folder is kept.
        fs::write(folder.join("unannounced.md"), "").unwrap();
        index.scan_changes(1, &[], std::slice::from_ref(&folder), 2);
        let status = index.status().unwrap();
        assert_eq!(status.count, 4);
        assert!(!status.scanning);
        let catalog = index.catalog.read().unwrap();
        assert!(catalog.entries.get(path_key(&folder).as_str()).is_some());
        assert!(catalog
            .entries
            .get(path_key(&folder.join("deep/kept.md")).as_str())
            .is_some());
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
            .sorted()
            .into_iter()
            .map(|entry| (entry.key.to_string(), (entry.is_dir, entry.size)))
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
        assert_eq!(entry.view().name(), "공문서 계획.hwpx");
        assert_eq!(entry.view().extension(), "hwpx");
        assert_eq!(entry.view().initials_name(), Some("ㄱㅁㅅ ㄱㅎ.hwpx"));
        assert_eq!(entry.result().name, "공문서 계획.HWPX");
        assert_eq!(
            Entry::new(raw("C:/a/.gitignore", 1), 1).view().extension(),
            ""
        );
        assert_eq!(
            Entry::new(raw("C:/a/archive.tar.gz", 1), 1)
                .view()
                .extension(),
            "gz"
        );
        let ascii = Entry::new(raw("C:/a/plain.txt", 1), 1);
        assert!(ascii.initials_path.is_none() && ascii.keyboard_path.is_none());
        // Stored entries come back field for field, and a scan view of a stored
        // entry reads exactly what the owned entry's view reads.
        let mut entries = Entries::new();
        let mut scratch = Scratch::default();
        for owned in [entry, ascii] {
            entries.insert(&owned);
            let stored = entries.get(&owned.key).unwrap();
            let copy = entries::owned(stored);
            assert_eq!(
                (
                    copy.path.as_ref(),
                    copy.key.as_ref(),
                    copy.search.as_deref()
                ),
                (
                    owned.path.as_ref(),
                    owned.key.as_ref(),
                    owned.search.as_deref()
                )
            );
            assert_eq!(copy.name_start, owned.name_start);
            assert_eq!(copy.initials_path, owned.initials_path);
            assert_eq!(copy.keyboard_path, owned.keyboard_path);
            let view = entries::view(stored, &mut scratch, true);
            let expected = owned.view();
            assert_eq!(
                (
                    view.searchable(),
                    view.name(),
                    view.initials_path,
                    view.keyboard_path
                ),
                (
                    expected.searchable(),
                    expected.name(),
                    expected.initials_path,
                    expected.keyboard_path
                )
            );
            assert_eq!(entries::result(stored).name, owned.result().name);
        }
    }
    #[test]
    fn verbatim_path_keys_preserve_filesystem_identity_and_root_boundaries() {
        let path = path_key(Path::new(r"\\?\C:\Mixed\file.txt\"));
        let unc = path_key(Path::new(r"\\?\UNC\Server\Share\"));
        #[cfg(windows)]
        {
            assert_eq!(path, "c:/mixed/file.txt");
            assert_eq!(unc, "//server/share");
        }
        #[cfg(not(windows))]
        {
            assert_eq!(path, "C:/Mixed/file.txt");
            assert_eq!(unc, "//Server/Share");
        }
        assert_ne!(
            path_key(Path::new("C:/e\u{301}.md")),
            path_key(Path::new("C:/é.md"))
        );
    }
    #[test]
    fn subtree_ranges_exclude_sibling_prefixes() {
        let mut entries = Entries::new();
        for path in [
            "C:/a",
            "C:/a/x.txt",
            "C:/a b/y.txt",
            "C:/a-b/z.txt",
            "C:/a/deep/w.txt",
            "C:/ab",
        ] {
            entries.insert(&Entry::new(raw(path, 1), 1));
        }
        let root = path_key(Path::new("C:/a"));
        let mut keys: Vec<_> = entries
            .subtree(&root)
            .map(|entry| entries::result(entry).name)
            .collect();
        keys.sort();
        assert_eq!(keys, ["a", "w.txt", "x.txt"]);
    }
    #[test]
    fn parallel_top_matches_equal_a_full_sort() {
        let owned: Vec<Entry> = (0..50_000)
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
        let mut entries = Entries::new();
        for entry in &owned {
            entries.insert(entry);
        }
        let rank = |entry: &View<'_>| {
            entry
                .name()
                .ends_with("-1.txt")
                .then(|| entry.name().as_bytes()[2] % 4)
        };
        let candidates = Candidates::All(entries.slots());
        let (total, top) = top_matches(&entries, &candidates, 50, false, rank);
        let mut expected: Vec<(u8, &str, &str)> = owned
            .iter()
            .filter_map(|entry| {
                let view = entry.view();
                rank(&view).map(|rank| (rank, view.name(), entry.key.as_ref()))
            })
            .collect();
        assert_eq!(total, expected.len());
        expected.sort();
        expected.truncate(50);
        let keys: Vec<&str> = top.iter().map(|item| item.key.as_ref()).collect();
        let expected: Vec<&str> = expected.iter().map(|item| item.2).collect();
        assert_eq!(keys, expected);
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
        // Every stored (re)build counts as an insertion; refreshes do not.
        let pointer = |index: &FileIndex| index.catalog.read().unwrap().entries.insertions();
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
            catalog.entries.insert(&ghost);
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
        // Every stored (re)build counts as an insertion; keeping does not.
        let pointer = |index: &FileIndex| index.catalog.read().unwrap().entries.insertions();
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
