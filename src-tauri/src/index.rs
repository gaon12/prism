use crate::filename_pattern::{initial_projection, FilenamePattern};
use crate::keyboard_pattern::{keyboard_projection, KeyboardPattern};
use crate::settings::SearchScope;
use chrono::{Datelike, Local, NaiveDate, TimeZone};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use std::{
    cmp::Ordering as CmpOrdering,
    collections::{BinaryHeap, HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Arc, Mutex, RwLock,
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

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileResult {
    pub path: String,
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified_ms: Option<i64>,
}
struct Entry {
    result: FileResult,
    key: String,
    name: String,
    searchable_path: String,
    extension: String,
    initials_path: Option<String>,
    initials_name: Option<String>,
    keyboard_path: Option<String>,
    seen: u64,
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
}
#[derive(Default)]
struct Catalog {
    entries: HashMap<String, Entry>,
    roots: Vec<Root>,
    status: IndexStatus,
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
            // Removed or disabled scopes must stop being searchable immediately.
            if clear {
                catalog.entries.clear();
                catalog.roots.clear();
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
        let pending = Arc::new(Mutex::new(Vec::<PathBuf>::new()));
        let overflow = Arc::new(AtomicBool::new(false));
        let mut watcher: Option<RecommendedWatcher> = None;
        let mut generation = 0;
        let mut scopes = vec![];
        let mut last_reconcile = SystemTime::now();
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
                    pending.lock().unwrap().clear();
                    overflow.store(false, Ordering::SeqCst);
                    let mut roots = vec![];
                    let mut warnings = vec![];
                    for scope in scopes.iter().filter(|scope| scope.filename_enabled) {
                        match fs::canonicalize(&scope.path) {
                            Ok(path) if path.is_dir() => roots.push(Root {
                                id: scope.id.clone(),
                                key: path_key(&path),
                                path,
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
                                    if paths.len() + event.paths.len() <= 4096 {
                                        paths.extend(event.paths);
                                    } else {
                                        dropped.store(true, Ordering::SeqCst);
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
                            for root in &roots {
                                if let Err(error) = next.watch(&root.path, RecursiveMode::Recursive)
                                {
                                    warnings.push(format!(
                                        "Live updates unavailable for {}: {error}",
                                        root.path.display()
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
                    self.scan(
                        generation,
                        &roots
                            .iter()
                            .map(|root| root.path.clone())
                            .collect::<Vec<_>>(),
                        true,
                    );
                    last_reconcile = SystemTime::now();
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            if generation == 0 || !self.current(generation) {
                continue;
            }
            // Periodic reconciliation also repairs missed network / OS watch events and missing roots.
            if overflow.swap(false, Ordering::SeqCst)
                || last_reconcile.elapsed().unwrap_or_default() > Duration::from_secs(120)
            {
                self.refresh(scopes.clone());
                continue;
            }
            let paths = std::mem::take(&mut *pending.lock().unwrap());
            if paths.is_empty() {
                continue;
            }
            let roots = match self.catalog.read() {
                Ok(catalog) => catalog.roots.clone(),
                Err(_) => continue,
            };
            let mut paths: Vec<_> = paths
                .into_iter()
                .filter(|path| roots.iter().any(|root| under(&path_key(path), &root.key)))
                .collect();
            paths.sort_by_key(|path| path.components().count());
            let mut unique = vec![];
            for path in paths {
                let key = path_key(&path);
                if !unique
                    .iter()
                    .any(|parent: &PathBuf| under(&key, &path_key(parent)))
                {
                    unique.push(path);
                }
            }
            if !unique.is_empty() {
                if let Ok(mut catalog) = self.catalog.write() {
                    catalog
                        .entries
                        .retain(|key, _| !unique.iter().any(|path| under(key, &path_key(path))));
                }
                self.scan(generation, &unique, false);
            }
        }
    }
    fn scan(&self, generation: u64, paths: &[PathBuf], reconcile: bool) {
        if !self.current(generation) {
            return;
        }
        if let Ok(mut catalog) = self.catalog.write() {
            catalog.status.scanning = true;
        }
        let mut batch = Vec::with_capacity(512);
        let mut skipped = 0;
        for path in paths {
            if !path.exists() {
                continue;
            }
            let walker = WalkDir::new(path)
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
                    return;
                }
                match entry {
                    Ok(entry) => {
                        if !entry.file_type().is_dir() && !entry.file_type().is_file() {
                            continue;
                        }
                        let metadata = match entry.metadata() {
                            Ok(metadata) => metadata,
                            Err(_) => {
                                skipped += 1;
                                continue;
                            }
                        };
                        let path = entry.path();
                        let key = path_key(path);
                        let name = entry.file_name().to_string_lossy().to_string();
                        batch.push(Entry {
                            keyboard_path: keyboard_projection(&normalized(&key)),
                            initials_path: initial_projection(&normalized(&key)),
                            initials_name: initial_projection(&normalized(&name)),
                            seen: generation,
                            result: FileResult {
                                path: display_path(path),
                                name: name.clone(),
                                is_dir: metadata.is_dir(),
                                size: if metadata.is_dir() { 0 } else { metadata.len() },
                                modified_ms: metadata
                                    .modified()
                                    .ok()
                                    .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                                    .map(|duration| duration.as_millis() as i64),
                            },
                            name: normalized(&name),
                            searchable_path: normalized(&key),
                            extension: normalized(
                                path.extension().and_then(|ext| ext.to_str()).unwrap_or(""),
                            ),
                            key,
                        });
                        if batch.len() >= 512 {
                            self.publish(generation, &mut batch, skipped, false);
                            skipped = 0;
                        }
                    }
                    Err(_) => skipped += 1,
                }
            }
        }
        self.publish(generation, &mut batch, skipped, !reconcile);
        if reconcile {
            if let Ok(mut catalog) = self.catalog.write() {
                if !self.current(generation) {
                    return;
                }
                catalog.entries.retain(|_, entry| entry.seen == generation);
                catalog.status.count = catalog.entries.len();
                catalog.status.scanning = false;
                catalog.status.revision += 1;
            }
        }
    }
    fn publish(&self, generation: u64, batch: &mut Vec<Entry>, skipped: usize, done: bool) {
        if let Ok(mut catalog) = self.catalog.write() {
            if !self.current(generation) {
                return;
            }
            for entry in batch.drain(..) {
                catalog.entries.insert(entry.key.clone(), entry);
            }
            catalog.status.count = catalog.entries.len();
            catalog.status.skipped += skipped;
            catalog.status.revision += 1;
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
        let root = scope_id.and_then(|id| catalog.roots.iter().find(|root| root.id == id));
        let mut heap = BinaryHeap::new();
        let mut total = 0;
        let mut keyboard_corrected = false;
        if scope_id.is_none() || root.is_some() {
            for entry in catalog.entries.values() {
                if root.is_some_and(|root| !under(&entry.key, &root.key)) || !query.matches(entry) {
                    continue;
                }
                total += 1;
                heap.push(Ranked {
                    rank: query.rank(entry),
                    entry,
                });
                if heap.len() > limit.clamp(1, 500) {
                    heap.pop();
                }
            }
        }
        if total == 0
            && allow_keyboard
            && query.keyboard_terms.iter().any(Option::is_some)
            && (scope_id.is_none() || root.is_some())
        {
            for entry in catalog.entries.values() {
                if root.is_some_and(|root| !under(&entry.key, &root.key))
                    || !query.matches_keyboard(entry)
                {
                    continue;
                }
                total += 1;
                heap.push(Ranked {
                    rank: if query.keyboard_name_matches(entry) {
                        6
                    } else {
                        7
                    },
                    entry,
                });
                if heap.len() > limit.clamp(1, 500) {
                    heap.pop();
                }
            }
            keyboard_corrected = total > 0;
        }
        Ok(SearchResponse {
            keyboard_corrected,
            results: heap
                .into_sorted_vec()
                .into_iter()
                .map(|ranked| ranked.entry.result.clone())
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
            || !catalog.entries.contains_key(&key)
            || !catalog.roots.iter().any(|root| under(&key, &root.key))
        {
            return Err(
                "This path is outside the active file index. Refresh and search again.".into(),
            );
        }
        Ok(canonical)
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
        (self.rank, &self.entry.name, &self.entry.key).cmp(&(
            other.rank,
            &other.entry.name,
            &other.entry.key,
        ))
    }
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
            .all(|term| term.matches(&entry.searchable_path, entry.initials_path.as_deref()))
            && self.filters_match(entry)
    }
    fn matches_keyboard(&self, entry: &Entry) -> bool {
        self.filters_match(entry)
            && self
                .terms
                .iter()
                .zip(&self.keyboard_terms)
                .all(|(term, keyboard)| {
                    term.matches(&entry.searchable_path, entry.initials_path.as_deref())
                        || keyboard.as_ref().is_some_and(|pattern| {
                            pattern.matches(&entry.searchable_path, entry.keyboard_path.as_deref())
                        })
                })
    }
    fn keyboard_name_matches(&self, entry: &Entry) -> bool {
        let projection = keyboard_projection(&entry.name);
        self.terms
            .iter()
            .zip(&self.keyboard_terms)
            .all(|(term, keyboard)| {
                term.matches(&entry.name, entry.initials_name.as_deref())
                    || keyboard
                        .as_ref()
                        .is_some_and(|pattern| pattern.matches(&entry.name, projection.as_deref()))
            })
    }
    fn filters_match(&self, entry: &Entry) -> bool {
        self.extensions
            .iter()
            .all(|set| !entry.result.is_dir && set.contains(&entry.extension))
            && self.dates.iter().all(|(start, end)| {
                entry
                    .result
                    .modified_ms
                    .is_some_and(|time| time >= *start && time < *end)
            })
    }
    fn rank(&self, entry: &Entry) -> u8 {
        if entry.name == self.exact {
            0
        } else if entry.name.starts_with(&self.exact) {
            1
        } else if self
            .terms
            .iter()
            .all(|term| term.literal_matches(&entry.name))
        {
            2
        } else if self
            .terms
            .iter()
            .all(|term| term.literal_matches(&entry.searchable_path))
        {
            3
        } else if self
            .terms
            .iter()
            .all(|term| term.matches(&entry.name, entry.initials_name.as_deref()))
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
        for name in ["가.txt", "가나.txt", "x가.txt", "가나다.txt", "가나다.txt"]
        {
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
}
