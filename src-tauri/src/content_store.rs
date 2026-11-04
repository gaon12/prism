//! Persistent local text repository. Every read that exposes content requires
//! the current authorized content roots; filename-only roots are insufficient.
use crate::document_extract::{ExtractedDocument, SourceFingerprint};
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Component, Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::{Duration, UNIX_EPOCH},
};
use unicode_normalization::UnicodeNormalization;

const CHUNK_CHARS: usize = 1200;
const OVERLAP_CHARS: usize = 150;
const MAX_TEXT_BYTES: usize = 32 * 1024 * 1024;
const MAX_SECTIONS: usize = 100_000;
const MAX_CHUNKS: usize = 100_000;
const MAX_QUERY_BYTES: usize = 4096;
const MAX_TERMS: usize = 32;
const MAX_RESULTS: usize = 100;
const MAX_IDS: usize = 256;
const MAX_CANDIDATES: usize = 250_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ContentStoreErrorCode {
    Cancelled,
    InvalidInput,
    OutsideScope,
    UnsafePath,
    SourceChanged,
    LimitExceeded,
    Storage,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentStoreError {
    pub code: ContentStoreErrorCode,
    pub message: String,
}
impl std::fmt::Display for ContentStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for ContentStoreError {}
type Result<T> = std::result::Result<T, ContentStoreError>;
fn error(code: ContentStoreErrorCode, message: &str) -> ContentStoreError {
    ContentStoreError {
        code,
        message: message.into(),
    }
}
fn storage(_: impl std::fmt::Debug) -> ContentStoreError {
    error(
        ContentStoreErrorCode::Storage,
        "Content repository operation failed",
    )
}
fn limit() -> ContentStoreError {
    error(
        ContentStoreErrorCode::LimitExceeded,
        "Content repository worker budget exceeded",
    )
}
fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        Err(error(
            ContentStoreErrorCode::Cancelled,
            "Content repository operation cancelled",
        ))
    } else {
        Ok(())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentSearchFilter {
    #[serde(default)]
    pub document_paths: Vec<String>,
    #[serde(default)]
    pub scope_paths: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentDocumentSummary {
    pub file_path: String,
    pub fingerprint: SourceFingerprint,
    pub format: String,
    pub section_count: usize,
    pub chunk_count: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentChunk {
    pub id: String,
    pub file_path: String,
    pub fingerprint: SourceFingerprint,
    pub section_index: u32,
    pub chunk_index: u32,
    pub text: String,
    pub page: Option<u32>,
    pub paragraph: Option<u32>,
    pub heading: Option<String>,
    pub char_start: u32,
    pub char_end: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentSearchHit {
    pub chunk: ContentChunk,
    pub score: f64,
    pub snippet: String,
}

pub struct ContentStore {
    connection: Mutex<Connection>,
}

fn normalized(text: &str) -> String {
    text.nfkc()
        .flat_map(char::to_lowercase)
        .filter(|ch| !ch.is_whitespace())
        .collect()
}

// Windows canonical paths use an extended prefix; persist a normal absolute
// spelling so purge also works after the source file/directory has disappeared.
fn normal_path(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let value = path.to_string_lossy();
        if let Some(rest) = value.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = value.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
    }
    path.to_owned()
}
fn path_key(path: &Path) -> String {
    let value = normal_path(path).to_string_lossy().into_owned();
    #[cfg(windows)]
    {
        value.to_lowercase()
    }
    #[cfg(not(windows))]
    {
        value
    }
}
fn within(path: &Path, root: &Path) -> bool {
    let mut components = path.components();
    for expected in root.components() {
        let Some(actual) = components.next() else {
            return false;
        };
        #[cfg(windows)]
        if actual.as_os_str().to_string_lossy().to_lowercase()
            != expected.as_os_str().to_string_lossy().to_lowercase()
        {
            return false;
        }
        #[cfg(not(windows))]
        if actual != expected {
            return false;
        }
    }
    true
}
fn no_links(path: &Path) -> bool {
    path.ancestors()
        .filter(|p| !p.as_os_str().is_empty())
        .all(|ancestor| {
            let Ok(meta) = fs::symlink_metadata(ancestor) else {
                return false;
            };
            if meta.file_type().is_symlink() {
                return false;
            }
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                if meta.file_attributes() & 0x400 != 0 {
                    return false;
                }
            }
            true
        })
}
fn safe_canonical(path: &Path, directory: bool) -> Option<PathBuf> {
    let absolute = std::path::absolute(path).ok()?;
    if !no_links(&absolute) {
        return None;
    }
    let canonical = absolute.canonicalize().ok()?;
    let meta = fs::metadata(&canonical).ok()?;
    if if directory {
        !meta.is_dir()
    } else {
        !meta.is_file()
    } {
        return None;
    }
    Some(normal_path(&canonical))
}
fn roots(paths: &[PathBuf]) -> Vec<PathBuf> {
    paths
        .iter()
        .filter_map(|p| safe_canonical(p, true))
        .collect()
}
fn modified(meta: &fs::Metadata) -> Option<u64> {
    meta.modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis()
        .try_into()
        .ok()
}
fn visible(path: &str, fingerprint: &SourceFingerprint, roots: &[PathBuf]) -> Option<PathBuf> {
    let canonical = safe_canonical(Path::new(path), false)?;
    if path_key(&canonical) != path_key(Path::new(path))
        || !roots.iter().any(|root| within(&canonical, root))
    {
        return None;
    }
    let meta = fs::metadata(&canonical).ok()?;
    if meta.len() != fingerprint.size_bytes
        || fingerprint
            .modified_unix_ms
            .is_some_and(|stamp| modified(&meta) != Some(stamp))
    {
        return None;
    }
    Some(canonical)
}

fn purge_path(path: &Path) -> Result<PathBuf> {
    let absolute = std::path::absolute(normal_path(path)).map_err(storage)?;
    let mut clean = PathBuf::new();
    for part in absolute.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                clean.pop();
            }
            _ => clean.push(part.as_os_str()),
        }
    }
    Ok(clean)
}

impl ContentStore {
    pub fn open(db_path: &Path) -> Result<Self> {
        if let Some(parent) = db_path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent).map_err(storage)?;
        }
        if db_path.exists() && !no_links(&std::path::absolute(db_path).map_err(storage)?) {
            return Err(error(
                ContentStoreErrorCode::UnsafePath,
                "Unsafe content repository path",
            ));
        }
        let connection = Connection::open(db_path).map_err(storage)?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(storage)?;
        let version: u32 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(storage)?;
        if version > 1 {
            return Err(error(
                ContentStoreErrorCode::Storage,
                "Content repository version is unsupported",
            ));
        }
        connection.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;
            BEGIN IMMEDIATE;
            CREATE TABLE IF NOT EXISTS content_documents (
                id INTEGER PRIMARY KEY, path_key TEXT NOT NULL UNIQUE, file_path TEXT NOT NULL,
                sha256 TEXT NOT NULL, size_bytes INTEGER NOT NULL, modified_unix_ms INTEGER,
                format TEXT NOT NULL, section_count INTEGER NOT NULL, chunk_count INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS content_sections (
                document_id INTEGER NOT NULL REFERENCES content_documents(id) ON DELETE CASCADE,
                section_index INTEGER NOT NULL, text TEXT NOT NULL, page INTEGER, paragraph INTEGER, heading TEXT,
                PRIMARY KEY(document_id, section_index)
            );
            CREATE TABLE IF NOT EXISTS content_chunks (
                id TEXT PRIMARY KEY, document_id INTEGER NOT NULL REFERENCES content_documents(id) ON DELETE CASCADE,
                section_index INTEGER NOT NULL, chunk_index INTEGER NOT NULL, char_start INTEGER NOT NULL, char_end INTEGER NOT NULL,
                text TEXT NOT NULL, normalized TEXT NOT NULL,
                FOREIGN KEY(document_id,section_index) REFERENCES content_sections(document_id,section_index) ON DELETE CASCADE
            );
            CREATE INDEX IF NOT EXISTS content_chunks_document ON content_chunks(document_id, section_index, chunk_index);
            PRAGMA user_version=1; COMMIT;
        ").map_err(storage)?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    /// Interrupt long SQLite statements when cancellation arrives. The monitor
    /// owns no document text and stops immediately after each operation.
    fn run<T>(
        &self,
        cancel: &AtomicBool,
        action: impl FnOnce(&mut Connection) -> Result<T>,
    ) -> Result<T> {
        check_cancel(cancel)?;
        let mut connection = self.connection.lock().map_err(storage)?;
        check_cancel(cancel)?;
        let interrupt = connection.get_interrupt_handle();
        let done = AtomicBool::new(false);
        std::thread::scope(|scope| {
            let done_ref = &done;
            let worker = scope.spawn(move || {
                while !done_ref.load(Ordering::Acquire) {
                    if cancel.load(Ordering::Relaxed) {
                        interrupt.interrupt();
                        break;
                    }
                    std::thread::park_timeout(Duration::from_millis(10));
                }
            });
            struct Finish<'a> {
                done: &'a AtomicBool,
                thread: std::thread::Thread,
            }
            impl Drop for Finish<'_> {
                fn drop(&mut self) {
                    self.done.store(true, Ordering::Release);
                    self.thread.unpark();
                }
            }
            let finish = Finish {
                done: &done,
                thread: worker.thread().clone(),
            };
            let result = action(&mut connection);
            drop(finish);
            worker.join().map_err(storage)?;
            if result.is_err() {
                check_cancel(cancel)?;
            }
            result
        })
    }

    pub fn replace_document(
        &self,
        document: &ExtractedDocument,
        allowed_content_roots: &[PathBuf],
        cancel: &AtomicBool,
    ) -> Result<ContentDocumentSummary> {
        check_cancel(cancel)?;
        if document.sections.len() > MAX_SECTIONS {
            return Err(limit());
        }
        if document.fingerprint.size_bytes > i64::MAX as u64
            || document
                .fingerprint
                .modified_unix_ms
                .is_some_and(|v| v > i64::MAX as u64)
            || document.fingerprint.sha256.len() != 64
            || !document
                .fingerprint
                .sha256
                .bytes()
                .all(|c| c.is_ascii_hexdigit())
        {
            return Err(error(
                ContentStoreErrorCode::InvalidInput,
                "Invalid extracted document metadata",
            ));
        }
        let allowed = roots(allowed_content_roots);
        let canonical = safe_canonical(Path::new(&document.file_path), false).ok_or_else(|| {
            error(
                ContentStoreErrorCode::UnsafePath,
                "Source document path is missing or unsafe",
            )
        })?;
        if !allowed.iter().any(|root| within(&canonical, root)) {
            return Err(error(
                ContentStoreErrorCode::OutsideScope,
                "Source document is outside authorized content roots",
            ));
        }
        if visible(
            canonical.to_string_lossy().as_ref(),
            &document.fingerprint,
            &allowed,
        )
        .is_none()
        {
            return Err(error(
                ContentStoreErrorCode::SourceChanged,
                "Source document changed after extraction",
            ));
        }
        let source_path = canonical.to_string_lossy().into_owned();
        let key = path_key(&canonical);
        let mut chunks = Vec::new();
        let mut total = 0usize;
        for (section_index, section) in document.sections.iter().enumerate() {
            check_cancel(cancel)?;
            total = total
                .checked_add(section.text.len())
                .and_then(|v| v.checked_add(section.heading.as_ref().map_or(0, String::len)))
                .ok_or_else(limit)?;
            if total > MAX_TEXT_BYTES {
                return Err(limit());
            }
            let chars: Vec<char> = section.text.chars().collect();
            if chars.iter().all(|c| c.is_whitespace()) {
                continue;
            }
            let mut start = 0;
            let mut chunk_index = 0;
            while start < chars.len() {
                check_cancel(cancel)?;
                if chunks.len() >= MAX_CHUNKS {
                    return Err(limit());
                }
                let end = (start + CHUNK_CHARS).min(chars.len());
                let text: String = chars[start..end].iter().collect();
                let mut digest = Sha256::new();
                for part in [&key, &document.fingerprint.sha256] {
                    digest.update((part.len() as u64).to_le_bytes());
                    digest.update(part.as_bytes());
                }
                digest.update((section_index as u64).to_le_bytes());
                digest.update((chunk_index as u64).to_le_bytes());
                digest.update((start as u64).to_le_bytes());
                digest.update(b"content-chunk-v1");
                digest.update(text.as_bytes());
                digest.update(section.page.unwrap_or(0).to_le_bytes());
                digest.update(section.paragraph.unwrap_or(0).to_le_bytes());
                if let Some(heading) = &section.heading {
                    digest.update(heading.as_bytes());
                }
                chunks.push(ContentChunk {
                    id: format!("chunk_{:x}", digest.finalize()),
                    file_path: source_path.clone(),
                    fingerprint: document.fingerprint.clone(),
                    section_index: section_index as u32,
                    chunk_index,
                    text,
                    page: section.page,
                    paragraph: section.paragraph,
                    heading: section.heading.clone(),
                    char_start: start as u32,
                    char_end: end as u32,
                });
                if end == chars.len() {
                    break;
                }
                start = end - OVERLAP_CHARS;
                chunk_index += 1;
            }
        }
        let summary = ContentDocumentSummary {
            file_path: source_path.clone(),
            fingerprint: document.fingerprint.clone(),
            format: document.format.clone(),
            section_count: document.sections.len(),
            chunk_count: chunks.len(),
        };
        self.run(cancel, |connection| {
            let tx = connection.transaction().map_err(storage)?;
            tx.execute("DELETE FROM content_documents WHERE path_key=?1", [&key]).map_err(storage)?;
            tx.execute("INSERT INTO content_documents(path_key,file_path,sha256,size_bytes,modified_unix_ms,format,section_count,chunk_count) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                params![key, source_path, document.fingerprint.sha256, document.fingerprint.size_bytes, document.fingerprint.modified_unix_ms, document.format, document.sections.len(), chunks.len()]).map_err(storage)?;
            let document_id = tx.last_insert_rowid();
            {
                let mut statement = tx.prepare("INSERT INTO content_sections(document_id,section_index,text,page,paragraph,heading) VALUES(?1,?2,?3,?4,?5,?6)").map_err(storage)?;
                for (index, section) in document.sections.iter().enumerate() { check_cancel(cancel)?; statement.execute(params![document_id,index,section.text,section.page,section.paragraph,section.heading]).map_err(storage)?; }
            }
            {
                let mut statement = tx.prepare("INSERT INTO content_chunks(id,document_id,section_index,chunk_index,char_start,char_end,text,normalized) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)").map_err(storage)?;
                for chunk in &chunks { check_cancel(cancel)?; statement.execute(params![chunk.id,document_id,chunk.section_index,chunk.chunk_index,chunk.char_start,chunk.char_end,chunk.text,normalized(&chunk.text)]).map_err(storage)?; }
            }
            check_cancel(cancel)?;
            if visible(&source_path, &document.fingerprint, &allowed).is_none() { return Err(error(ContentStoreErrorCode::SourceChanged, "Source document changed during content indexing")); }
            tx.commit().map_err(storage)?;
            Ok(summary)
        })
    }

    pub fn search(
        &self,
        query: &str,
        filter: &ContentSearchFilter,
        allowed_content_roots: &[PathBuf],
        limit_count: usize,
        cancel: &AtomicBool,
    ) -> Result<Vec<ContentSearchHit>> {
        check_cancel(cancel)?;
        if query.len() > MAX_QUERY_BYTES
            || !(1..=MAX_RESULTS).contains(&limit_count)
            || filter.document_paths.len() > MAX_IDS
            || filter.scope_paths.len() > MAX_IDS
        {
            return Err(error(
                ContentStoreErrorCode::InvalidInput,
                "Content search request exceeds allowed limits",
            ));
        }
        let mut seen = HashSet::new();
        let terms: Vec<String> = query
            .split_whitespace()
            .map(normalized)
            .filter(|term| !term.is_empty() && seen.insert(term.clone()))
            .collect();
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        if terms.len() > MAX_TERMS {
            return Err(error(
                ContentStoreErrorCode::InvalidInput,
                "Content search has too many terms",
            ));
        }
        let allowed = roots(allowed_content_roots);
        if allowed.is_empty() {
            return Ok(Vec::new());
        }
        let documents: HashSet<String> = filter
            .document_paths
            .iter()
            .filter_map(|p| safe_canonical(Path::new(p), false))
            .map(|p| path_key(&p))
            .collect();
        let scopes: Vec<PathBuf> = filter
            .scope_paths
            .iter()
            .filter_map(|p| safe_canonical(Path::new(p), true))
            .collect();
        if (!filter.document_paths.is_empty() && documents.is_empty())
            || (!filter.scope_paths.is_empty() && scopes.is_empty())
        {
            return Ok(Vec::new());
        }
        self.run(cancel, |connection| {
            let conditions = (1..=terms.len())
                .map(|n| format!("instr(c.normalized,?{n})>0"))
                .collect::<Vec<_>>()
                .join(" AND ");
            let sql = format!(
                "{CHUNK_SELECT} WHERE {conditions} ORDER BY c.rowid LIMIT {}",
                MAX_CANDIDATES + 1
            );
            let mut statement = connection.prepare(&sql).map_err(storage)?;
            let mut rows = statement
                .query(params_from_iter(terms.iter()))
                .map_err(storage)?;
            let mut hits = Vec::new();
            let mut visibility = HashMap::new();
            let phrase = normalized(query);
            let mut candidates = 0;
            while let Some(row) = rows.next().map_err(storage)? {
                check_cancel(cancel)?;
                candidates += 1;
                if candidates > MAX_CANDIDATES {
                    return Err(limit());
                }
                let chunk = read_chunk(row).map_err(storage)?;
                let permitted = *visibility
                    .entry(chunk.file_path.clone())
                    .or_insert_with(|| {
                        let Some(path) = visible(&chunk.file_path, &chunk.fingerprint, &allowed)
                        else {
                            return false;
                        };
                        (documents.is_empty() || documents.contains(&path_key(&path)))
                            && (scopes.is_empty()
                                || scopes.iter().any(|scope| within(&path, scope)))
                    });
                if !permitted {
                    continue;
                }
                let searchable = normalized(&chunk.text);
                let score = terms
                    .iter()
                    .map(|term| searchable.match_indices(term).take(8).count() as f64 * 10.0)
                    .sum::<f64>()
                    + if searchable.contains(&phrase) {
                        25.0
                    } else {
                        0.0
                    };
                let snippet = plain_snippet(&chunk.text, &terms);
                hits.push(ContentSearchHit {
                    chunk,
                    score,
                    snippet,
                });
                hits.sort_by(|a, b| {
                    b.score
                        .total_cmp(&a.score)
                        .then_with(|| a.chunk.file_path.cmp(&b.chunk.file_path))
                        .then_with(|| a.chunk.section_index.cmp(&b.chunk.section_index))
                        .then_with(|| a.chunk.chunk_index.cmp(&b.chunk.chunk_index))
                });
                hits.truncate(limit_count);
            }
            check_cancel(cancel)?;
            hits.retain(|hit| {
                visible(&hit.chunk.file_path, &hit.chunk.fingerprint, &allowed).is_some()
            });
            Ok(hits)
        })
    }

    pub fn chunks_by_ids(
        &self,
        ids: &[String],
        allowed_content_roots: &[PathBuf],
        cancel: &AtomicBool,
    ) -> Result<Vec<ContentChunk>> {
        check_cancel(cancel)?;
        if ids.len() > MAX_IDS || ids.iter().any(|id| id.len() > 128) {
            return Err(error(
                ContentStoreErrorCode::InvalidInput,
                "Content chunk lookup exceeds allowed limits",
            ));
        }
        let allowed = roots(allowed_content_roots);
        if allowed.is_empty() || ids.is_empty() {
            return Ok(Vec::new());
        }
        self.run(cancel, |connection| {
            let mut statement = connection
                .prepare(&format!("{CHUNK_SELECT} WHERE c.id=?1"))
                .map_err(storage)?;
            let mut output = Vec::new();
            let mut seen = HashSet::new();
            for id in ids {
                check_cancel(cancel)?;
                if !seen.insert(id) {
                    continue;
                }
                let chunk = statement
                    .query_row([id], read_chunk)
                    .optional()
                    .map_err(storage)?;
                if let Some(chunk) = chunk {
                    if visible(&chunk.file_path, &chunk.fingerprint, &allowed).is_some() {
                        output.push(chunk);
                    }
                }
            }
            Ok(output)
        })
    }

    /// Enumerate the complete current chunk set for one authorized source. A
    /// missing/unsafe/out-of-scope path returns no rows, never a broader scan.
    pub fn chunks_for_document(
        &self,
        path: &Path,
        allowed_content_roots: &[PathBuf],
        cancel: &AtomicBool,
    ) -> Result<Vec<ContentChunk>> {
        check_cancel(cancel)?;
        let allowed = roots(allowed_content_roots);
        let Some(canonical) = safe_canonical(path, false) else {
            return Ok(Vec::new());
        };
        if !allowed.iter().any(|root| within(&canonical, root)) {
            return Ok(Vec::new());
        }
        let key = path_key(&canonical);
        self.run(cancel, |connection| {
            let sql = format!("{CHUNK_SELECT} WHERE d.path_key=?1 ORDER BY c.section_index,c.chunk_index LIMIT {}", MAX_CHUNKS + 1);
            let mut statement = connection.prepare(&sql).map_err(storage)?;
            let mut rows = statement.query([key]).map_err(storage)?;
            let mut chunks = Vec::new();
            while let Some(row) = rows.next().map_err(storage)? {
                check_cancel(cancel)?;
                if chunks.len() == MAX_CHUNKS { return Err(limit()); }
                chunks.push(read_chunk(row).map_err(storage)?);
            }
            if chunks.first().is_some_and(|chunk| visible(&chunk.file_path, &chunk.fingerprint, &allowed).is_none()) { chunks.clear(); }
            check_cancel(cancel)?;
            Ok(chunks)
        })
    }

    pub fn purge_document(&self, path: &Path, cancel: &AtomicBool) -> Result<usize> {
        let key = path_key(&purge_path(path)?);
        self.run(cancel, |connection| {
            let tx = connection.transaction().map_err(storage)?;
            let count = tx
                .execute("DELETE FROM content_documents WHERE path_key=?1", [key])
                .map_err(storage)?;
            check_cancel(cancel)?;
            tx.commit().map_err(storage)?;
            Ok(count)
        })
    }
    pub fn purge_scope(&self, path: &Path, cancel: &AtomicBool) -> Result<usize> {
        let scope = purge_path(path)?;
        self.purge_where(cancel, |path, _| within(Path::new(path), &scope))
    }
    pub fn purge_outside_roots(
        &self,
        allowed_content_roots: &[PathBuf],
        cancel: &AtomicBool,
    ) -> Result<usize> {
        let allowed = roots(allowed_content_roots);
        self.purge_where(cancel, |path, fingerprint| {
            visible(path, fingerprint, &allowed).is_none()
        })
    }
    fn purge_where(
        &self,
        cancel: &AtomicBool,
        remove: impl Fn(&str, &SourceFingerprint) -> bool,
    ) -> Result<usize> {
        self.run(cancel, |connection| {
            let tx = connection.transaction().map_err(storage)?;
            let mut ids = Vec::new();
            {
                let mut statement = tx.prepare("SELECT id,file_path,sha256,size_bytes,modified_unix_ms FROM content_documents").map_err(storage)?;
                let mut rows = statement.query([]).map_err(storage)?;
                while let Some(row) = rows.next().map_err(storage)? {
                    check_cancel(cancel)?;
                    let path: String = row.get(1).map_err(storage)?;
                    let fingerprint = SourceFingerprint { sha256: row.get(2).map_err(storage)?, size_bytes: row.get(3).map_err(storage)?, modified_unix_ms: row.get(4).map_err(storage)? };
                    if remove(&path, &fingerprint) { ids.push(row.get::<_,i64>(0).map_err(storage)?); }
                }
            }
            for id in &ids { check_cancel(cancel)?; tx.execute("DELETE FROM content_documents WHERE id=?1", [id]).map_err(storage)?; }
            check_cancel(cancel)?;
            tx.commit().map_err(storage)?;
            Ok(ids.len())
        })
    }
}

const CHUNK_SELECT: &str = "SELECT c.id,d.file_path,d.sha256,d.size_bytes,d.modified_unix_ms,c.section_index,c.chunk_index,c.text,s.page,s.paragraph,s.heading,c.char_start,c.char_end FROM content_chunks c JOIN content_documents d ON d.id=c.document_id JOIN content_sections s ON s.document_id=c.document_id AND s.section_index=c.section_index";
fn read_chunk(row: &rusqlite::Row<'_>) -> rusqlite::Result<ContentChunk> {
    Ok(ContentChunk {
        id: row.get(0)?,
        file_path: row.get(1)?,
        fingerprint: SourceFingerprint {
            sha256: row.get(2)?,
            size_bytes: row.get(3)?,
            modified_unix_ms: row.get(4)?,
        },
        section_index: row.get(5)?,
        chunk_index: row.get(6)?,
        text: row.get(7)?,
        page: row.get(8)?,
        paragraph: row.get(9)?,
        heading: row.get(10)?,
        char_start: row.get(11)?,
        char_end: row.get(12)?,
    })
}
fn plain_snippet(text: &str, terms: &[String]) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= 320 {
        return text.to_owned();
    }
    // Locate a window without inventing byte offsets after NFKC expansion or
    // combining-mark composition. Text remains unchanged in every window.
    let mut selected = 0;
    for start in (0..chars.len()).step_by(160) {
        let end = (start + 320).min(chars.len());
        let window: String = chars[start..end].iter().collect();
        let norm = normalized(&window);
        if terms.iter().any(|term| norm.contains(term)) {
            selected = start;
            break;
        }
    }
    let end = (selected + 320).min(chars.len());
    chars[selected..end].iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document_extract::{extract_document, ExtractOptions};

    fn cancel() -> AtomicBool {
        AtomicBool::new(false)
    }
    fn document(dir: &Path, name: &str, text: &str) -> ExtractedDocument {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, text).unwrap();
        extract_document(&path, &ExtractOptions::default(), &cancel()).unwrap()
    }
    fn search(store: &ContentStore, query: &str, roots: &[PathBuf]) -> Vec<ContentSearchHit> {
        store
            .search(query, &ContentSearchFilter::default(), roots, 20, &cancel())
            .unwrap()
    }

    #[test]
    fn restart_preserves_source_metadata_and_chunk_ids() {
        let dir = tempfile::tempdir().unwrap();
        let roots = vec![dir.path().to_owned()];
        let mut doc = document(
            dir.path(),
            "source.md",
            "# 매출 보고\n한국어 프로젝트 분기 매출 42\n",
        );
        doc.sections[1].page = Some(3);
        let db = dir.path().join("index.sqlite");
        let store = ContentStore::open(&db).unwrap();
        let summary = store.replace_document(&doc, &roots, &cancel()).unwrap();
        assert_eq!(summary.section_count, 2);
        let original = search(&store, "프로젝트 42", &roots).remove(0);
        drop(store);
        let store = ContentStore::open(&db).unwrap();
        let hit = search(&store, "프로젝트 42", &roots).remove(0);
        assert_eq!(hit.chunk.id, original.chunk.id);
        assert_eq!(hit.chunk.page, Some(3));
        assert_eq!(hit.chunk.paragraph, Some(2));
        assert_eq!(hit.chunk.heading.as_deref(), Some("매출 보고"));
        assert_eq!(hit.chunk.fingerprint, doc.fingerprint);
        assert_eq!(
            store
                .chunks_by_ids(&[hit.chunk.id], &roots, &cancel())
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn unicode_normalization_whitespace_and_partial_and_queries() {
        let dir = tempfile::tempdir().unwrap();
        let roots = vec![dir.path().to_owned()];
        let store = ContentStore::open(&dir.path().join("index.sqlite")).unwrap();
        let doc = document(
            dir.path(),
            "source.txt",
            "한 국 어 프로젝트 ＡＢＣ１２３ mixedCase\n다른 줄 only",
        );
        store.replace_document(&doc, &roots, &cancel()).unwrap();
        assert_eq!(search(&store, "한국어 abc123 MIXED", &roots).len(), 1);
        assert_eq!(search(&store, "국어 프로", &roots).len(), 1);
        assert!(search(&store, "프로젝트 없는단어", &roots).is_empty());
        assert!(search(&store, "프로젝트 only", &roots).is_empty()); // section boundary is preserved
    }

    #[test]
    fn scalar_chunks_preserve_overlap_and_section_boundaries() {
        let dir = tempfile::tempdir().unwrap();
        let roots = vec![dir.path().to_owned()];
        let store = ContentStore::open(&dir.path().join("index.sqlite")).unwrap();
        let text = format!("{}\n표제\t셀값", "가🙂e\u{301}".repeat(700));
        let doc = document(dir.path(), "source.txt", &text);
        let summary = store.replace_document(&doc, &roots, &cancel()).unwrap();
        assert_eq!(summary.chunk_count, 4);
        let mut chunks: Vec<_> = search(&store, "가", &roots)
            .into_iter()
            .map(|hit| hit.chunk)
            .collect();
        chunks.sort_by_key(|chunk| chunk.chunk_index);
        assert_eq!(chunks[0].char_start, 0);
        assert_eq!(chunks[0].char_end, 1200);
        assert_eq!(chunks[1].char_start, 1050);
        let tail: String = chunks[0].text.chars().skip(1050).collect();
        let head: String = chunks[1].text.chars().take(150).collect();
        assert_eq!(tail, head);
        assert!(chunks
            .iter()
            .all(|chunk| chunk.text.chars().count() <= CHUNK_CHARS && chunk.section_index == 0));
        assert_eq!(search(&store, "표제", &roots)[0].chunk.text, "표제\t셀값");
    }

    #[test]
    fn replacement_invalidates_old_ids_without_duplicate_chunks() {
        let dir = tempfile::tempdir().unwrap();
        let roots = vec![dir.path().to_owned()];
        let store = ContentStore::open(&dir.path().join("index.sqlite")).unwrap();
        let doc = document(dir.path(), "source.txt", "old content");
        store.replace_document(&doc, &roots, &cancel()).unwrap();
        let old = search(&store, "old", &roots)[0].chunk.id.clone();
        let doc = document(dir.path(), "source.txt", "replacement content longer");
        store.replace_document(&doc, &roots, &cancel()).unwrap();
        assert!(search(&store, "old", &roots).is_empty());
        assert!(store
            .chunks_by_ids(&[old], &roots, &cancel())
            .unwrap()
            .is_empty());
        assert_eq!(search(&store, "replacement", &roots).len(), 1);
        store.replace_document(&doc, &roots, &cancel()).unwrap();
        assert_eq!(search(&store, "replacement", &roots).len(), 1);
    }

    #[test]
    fn scope_filters_and_current_authorization_intersect() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        let store = ContentStore::open(&dir.path().join("index.sqlite")).unwrap();
        let first = document(&a, "source.txt", "shared search");
        let second = document(&b, "source.txt", "shared search");
        let all = vec![dir.path().to_owned()];
        for doc in [&first, &second] {
            store.replace_document(doc, &all, &cancel()).unwrap();
        }
        assert_eq!(search(&store, "shared", &[a.clone()]).len(), 1);
        assert!(search(&store, "shared", &[]).is_empty());
        let filter = ContentSearchFilter {
            document_paths: vec![second.file_path.clone()],
            scope_paths: vec![],
        };
        assert!(store
            .search("shared", &filter, &[a.clone()], 20, &cancel())
            .unwrap()
            .is_empty());
        let filter = ContentSearchFilter {
            document_paths: vec![],
            scope_paths: vec![a.to_string_lossy().into_owned()],
        };
        assert_eq!(
            store
                .search("shared", &filter, &all, 20, &cancel())
                .unwrap()
                .len(),
            1
        );
        let filter = ContentSearchFilter {
            document_paths: vec![dir.path().join("missing").to_string_lossy().into_owned()],
            scope_paths: vec![],
        };
        assert!(store
            .search("shared", &filter, &all, 20, &cancel())
            .unwrap()
            .is_empty());
        let id = search(&store, "shared", &[b])[0].chunk.id.clone();
        assert!(store
            .chunks_by_ids(&[id], &[a], &cancel())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn deletion_and_metadata_changes_hide_stale_sources() {
        let dir = tempfile::tempdir().unwrap();
        let roots = vec![dir.path().to_owned()];
        let store = ContentStore::open(&dir.path().join("index.sqlite")).unwrap();
        let doc = document(dir.path(), "source.txt", "sensitive source");
        store.replace_document(&doc, &roots, &cancel()).unwrap();
        let id = search(&store, "sensitive", &roots)[0].chunk.id.clone();
        fs::write(&doc.file_path, "changed").unwrap();
        assert!(search(&store, "sensitive", &roots).is_empty());
        assert!(store
            .chunks_by_ids(&[id], &roots, &cancel())
            .unwrap()
            .is_empty());
        assert_eq!(
            store
                .replace_document(&doc, &roots, &cancel())
                .unwrap_err()
                .code,
            ContentStoreErrorCode::SourceChanged
        );
        fs::remove_file(&doc.file_path).unwrap();
        assert_eq!(
            store
                .purge_document(Path::new(&doc.file_path), &cancel())
                .unwrap(),
            1
        );
    }

    #[test]
    fn purge_scope_uses_components_and_cascades_sections_and_chunks() {
        let dir = tempfile::tempdir().unwrap();
        let roots = vec![dir.path().to_owned()];
        let store = ContentStore::open(&dir.path().join("index.sqlite")).unwrap();
        for name in ["folder/a.txt", "folder/sub/b.txt", "folder-other/c.txt"] {
            let doc = document(dir.path(), name, "shared");
            store.replace_document(&doc, &roots, &cancel()).unwrap();
        }
        assert_eq!(
            store
                .purge_scope(&dir.path().join("folder"), &cancel())
                .unwrap(),
            2
        );
        assert_eq!(search(&store, "shared", &roots).len(), 1);
        assert_eq!(store.purge_outside_roots(&[], &cancel()).unwrap(), 1);
        let connection = store.connection.lock().unwrap();
        for table in ["content_documents", "content_sections", "content_chunks"] {
            let count: i64 = connection
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0);
        }
    }

    #[test]
    fn failed_replacement_rolls_back_and_cancel_does_not_delete() {
        let dir = tempfile::tempdir().unwrap();
        let roots = vec![dir.path().to_owned()];
        let store = ContentStore::open(&dir.path().join("index.sqlite")).unwrap();
        let doc = document(dir.path(), "source.txt", "original");
        store.replace_document(&doc, &roots, &cancel()).unwrap();
        store.connection.lock().unwrap().execute_batch("CREATE TRIGGER reject_chunk BEFORE INSERT ON content_chunks BEGIN SELECT RAISE(ABORT,'fixture failure'); END;").unwrap();
        assert_eq!(
            store
                .replace_document(&doc, &roots, &cancel())
                .unwrap_err()
                .code,
            ContentStoreErrorCode::Storage
        );
        assert_eq!(search(&store, "original", &roots).len(), 1);
        let cancelled = AtomicBool::new(true);
        assert_eq!(
            store.purge_scope(dir.path(), &cancelled).unwrap_err().code,
            ContentStoreErrorCode::Cancelled
        );
        assert_eq!(search(&store, "original", &roots).len(), 1);
    }

    #[test]
    fn document_chunk_enumeration_is_complete_ordered_and_authorized() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("allowed");
        let outside = dir.path().join("outside");
        let authorized = vec![root.clone()];
        let store = ContentStore::open(&dir.path().join("index.sqlite")).unwrap();
        let doc = document(
            &root,
            "source.txt",
            &format!("{}\nother section", "한글🙂".repeat(900)),
        );
        let summary = store
            .replace_document(&doc, &authorized, &cancel())
            .unwrap();
        let chunks = store
            .chunks_for_document(Path::new(&doc.file_path), &authorized, &cancel())
            .unwrap();
        assert_eq!(chunks.len(), summary.chunk_count);
        assert!(chunks
            .windows(2)
            .all(|pair| (pair[0].section_index, pair[0].chunk_index)
                < (pair[1].section_index, pair[1].chunk_index)));
        assert!(store
            .chunks_for_document(&root.join("missing.txt"), &authorized, &cancel())
            .unwrap()
            .is_empty());
        assert!(store
            .chunks_for_document(Path::new(&doc.file_path), &[], &cancel())
            .unwrap()
            .is_empty());
        let other = document(&outside, "other.txt", "outside fixture");
        store
            .replace_document(&other, &[dir.path().to_owned()], &cancel())
            .unwrap();
        assert!(store
            .chunks_for_document(Path::new(&other.file_path), &authorized, &cancel())
            .unwrap()
            .is_empty());
        let cancelled = AtomicBool::new(true);
        assert_eq!(
            store
                .chunks_for_document(Path::new(&doc.file_path), &authorized, &cancelled)
                .unwrap_err()
                .code,
            ContentStoreErrorCode::Cancelled
        );
        fs::write(&doc.file_path, "changed").unwrap();
        assert!(store
            .chunks_for_document(Path::new(&doc.file_path), &authorized, &cancel())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn cancellation_interrupts_sql_and_rolls_back_before_join() {
        let dir = tempfile::tempdir().unwrap();
        let authorized = vec![dir.path().to_owned()];
        let store = ContentStore::open(&dir.path().join("index.sqlite")).unwrap();
        let doc = document(dir.path(), "source.txt", "original content");
        store
            .replace_document(&doc, &authorized, &cancel())
            .unwrap();
        let cancelled = AtomicBool::new(false);
        let started = std::time::Instant::now();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(20));
                cancelled.store(true, Ordering::Release);
            });
            let result = store.run(&cancelled, |connection| -> Result<()> {
                let transaction = connection.transaction().map_err(storage)?;
                transaction.execute("DELETE FROM content_documents", []).map_err(storage)?;
                let _: i64 = transaction.query_row("WITH RECURSIVE counter(x) AS (VALUES(0) UNION ALL SELECT x+1 FROM counter WHERE x<1000000000) SELECT sum(x) FROM counter", [], |row| row.get(0)).map_err(storage)?;
                transaction.commit().map_err(storage)?;
                Ok(())
            });
            assert_eq!(result.unwrap_err().code, ContentStoreErrorCode::Cancelled);
        });
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(search(&store, "original", &authorized).len(), 1);
    }

    #[test]
    fn cancellation_monitor_is_joined_when_an_operation_unwinds() {
        let dir = tempfile::tempdir().unwrap();
        let store = ContentStore::open(&dir.path().join("index.sqlite")).unwrap();
        let started = std::time::Instant::now();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            store.run(&cancel(), |_| -> Result<()> {
                panic!("synthetic operation failure")
            })
        }));
        assert!(result.is_err());
        assert!(started.elapsed() < Duration::from_secs(5));
        drop(store);
    }

    #[test]
    fn invalid_requests_are_bounded_and_snippets_are_plain_text() {
        let dir = tempfile::tempdir().unwrap();
        let roots = vec![dir.path().to_owned()];
        let store = ContentStore::open(&dir.path().join("index.sqlite")).unwrap();
        let doc = document(
            dir.path(),
            "source.txt",
            "<script>alert('fixture')</script> & original",
        );
        store.replace_document(&doc, &roots, &cancel()).unwrap();
        assert_eq!(
            search(&store, "original", &roots)[0].snippet,
            "<script>alert('fixture')</script> & original"
        );
        assert_eq!(
            store
                .search(
                    &"x".repeat(4097),
                    &ContentSearchFilter::default(),
                    &roots,
                    20,
                    &cancel()
                )
                .unwrap_err()
                .code,
            ContentStoreErrorCode::InvalidInput
        );
        assert_eq!(
            store
                .chunks_by_ids(&vec!["id".into(); 257], &roots, &cancel())
                .unwrap_err()
                .code,
            ContentStoreErrorCode::InvalidInput
        );
        assert_eq!(
            store
                .replace_document(&doc, &[], &cancel())
                .unwrap_err()
                .code,
            ContentStoreErrorCode::OutsideScope
        );
    }
}
