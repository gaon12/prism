//! Persistent local text repository. Every read that exposes content requires
//! the current authorized content roots; filename-only roots are insufficient.
use crate::document_extract::{ExtractedDocument, SourceFingerprint};
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
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
/// Most extensions one search may filter on (e.g. "pdf", "docx").
const MAX_EXTENSIONS: usize = 16;
/// Longest accepted extension, without the dot.
const MAX_EXTENSION_LEN: usize = 16;
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
/// Keeps a caller's write failure intact while a read transaction is unwound.
#[derive(Debug)]
pub(crate) enum SnapshotError<E, S = ContentStoreError> {
    Source(S),
    Visitor(E),
}
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
    /// Only search files with one of these extensions, e.g. `["pdf", ".DOCX"]`.
    /// Empty means "any extension". A leading dot and upper/lower case do not matter.
    /// Unlike `document_paths`, this has no file-count limit: it is checked inside the query,
    /// so a filter matching thousands of PDFs costs the same as one matching a single file.
    #[serde(default)]
    pub extensions: Vec<String>,
}

/// Cleans up the extension filter: strips one leading dot, lower-cases, and removes
/// duplicates. Only ASCII letters and digits are accepted, so the values are safe to put in a
/// SQL `LIKE` pattern (no `%` or `_` wildcards can sneak in).
pub(crate) fn normalized_extensions(extensions: &[String]) -> Result<Vec<String>> {
    if extensions.len() > MAX_EXTENSIONS {
        return Err(error(
            ContentStoreErrorCode::InvalidInput,
            "Content search has too many extension filters",
        ));
    }
    let mut clean: Vec<String> = Vec::new();
    for extension in extensions {
        let value = extension.trim();
        let value = value
            .strip_prefix('.')
            .unwrap_or(value)
            .to_ascii_lowercase();
        let valid = !value.is_empty()
            && value.len() <= MAX_EXTENSION_LEN
            && value.bytes().all(|b| b.is_ascii_alphanumeric());
        if !valid {
            return Err(error(
                ContentStoreErrorCode::InvalidInput,
                "Content search extension filter is invalid",
            ));
        }
        if !clean.contains(&value) {
            clean.push(value);
        }
    }
    Ok(clean)
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentDocumentSummary {
    pub file_path: String,
    pub fingerprint: SourceFingerprint,
    pub format: String,
    pub section_count: usize,
    pub chunk_count: usize,
    pub extractor_version: Option<String>,
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

/// Identity for physical cache maintenance only. There is deliberately no text
/// or serialization surface: this is never permission to serve a source.
#[derive(Debug)]
pub(crate) struct CachedChunkIdentity {
    pub id: String,
    pub file_path: String,
    pub fingerprint: SourceFingerprint,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentSearchHit {
    pub chunk: ContentChunk,
    pub score: f64,
    pub snippet: String,
}
/// Search results plus numbers that explain how complete they are.
///
/// A search first collects every chunk whose text contains all terms ("candidates"), then keeps
/// only the ones the caller may see. To bound memory and time, at most `candidate_limit`
/// candidates are collected. If more existed, `hard_limit_reached` is true: the hits are then
/// only a partial answer, and an empty list does NOT mean "nothing matches". The UI should say
/// so and suggest a more specific query.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentSearchResponse {
    pub hits: Vec<ContentSearchHit>,
    /// How many candidates were collected (before the scope and permission checks).
    pub candidate_count: usize,
    pub candidate_limit: usize,
    pub hard_limit_reached: bool,
}
impl ContentSearchResponse {
    fn empty(candidate_limit: usize) -> Self {
        Self {
            hits: Vec::new(),
            candidate_count: 0,
            candidate_limit,
            hard_limit_reached: false,
        }
    }
}

pub struct ContentStore {
    connection: Mutex<Connection>,
    path: PathBuf,
    /// Maximum candidates collected per search. Always `MAX_CANDIDATES` in the app; tests lower
    /// it to exercise the partial-result path without creating 250,000 chunks.
    candidate_limit: usize,
}

fn connect(db_path: &Path) -> rusqlite::Result<Connection> {
    let connection = Connection::open(db_path)?;
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=NORMAL;")?;
    Ok(connection)
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
    crate::path_identity::no_links(path).is_ok()
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

/// Cache cleanup needs stronger evidence than a read being unavailable. Search
/// still uses `visible` above and exposes nothing on any validation failure.
/// Only an authorized, unchanged source may recover its cached extraction after
/// a transient I/O failure; an absent scope never gets this grace period.
fn cached_source_is_obsolete(
    path: &str,
    fingerprint: &SourceFingerprint,
    roots: &[PathBuf],
) -> bool {
    let stored = normal_path(Path::new(path));
    if !roots.iter().any(|root| within(&stored, root)) {
        return true;
    }
    let inspect = || -> std::io::Result<bool> {
        let absolute = std::path::absolute(&stored)?;
        crate::path_identity::no_links(&absolute)?;
        let canonical = normal_path(&absolute.canonicalize()?);
        if path_key(&canonical) != path_key(&stored)
            || !roots.iter().any(|root| within(&canonical, root))
        {
            return Ok(true);
        }
        let meta = fs::metadata(&canonical)?;
        Ok(!meta.is_file()
            || meta.len() != fingerprint.size_bytes
            || fingerprint
                .modified_unix_ms
                .is_some_and(|expected| modified(&meta).is_some_and(|current| current != expected)))
    };
    match inspect() {
        Ok(obsolete) => obsolete,
        Err(failure) => matches!(
            failure.kind(),
            std::io::ErrorKind::NotFound
                | std::io::ErrorKind::NotADirectory
                | std::io::ErrorKind::InvalidInput
        ),
    }
}

fn purge_path(path: &Path) -> Result<PathBuf> {
    crate::path_identity::comparison(path).map_err(|failure| {
        if failure.kind() == std::io::ErrorKind::InvalidInput {
            error(
                ContentStoreErrorCode::UnsafePath,
                "Unsafe cache comparison path",
            )
        } else {
            storage(failure)
        }
    })
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
        if version > 2 {
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
            CREATE TABLE IF NOT EXISTS content_extraction_versions (
                document_id INTEGER PRIMARY KEY REFERENCES content_documents(id) ON DELETE CASCADE,
                version TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS content_chunks (
                id TEXT PRIMARY KEY, document_id INTEGER NOT NULL REFERENCES content_documents(id) ON DELETE CASCADE,
                section_index INTEGER NOT NULL, chunk_index INTEGER NOT NULL, char_start INTEGER NOT NULL, char_end INTEGER NOT NULL,
                text TEXT NOT NULL, normalized TEXT NOT NULL,
                FOREIGN KEY(document_id,section_index) REFERENCES content_sections(document_id,section_index) ON DELETE CASCADE
            );
            CREATE INDEX IF NOT EXISTS content_chunks_document ON content_chunks(document_id, section_index, chunk_index);
            CREATE VIRTUAL TABLE IF NOT EXISTS content_chunks_fts USING fts5(
                normalized, content='content_chunks', content_rowid='rowid', tokenize='trigram'
            );
            CREATE TRIGGER IF NOT EXISTS content_chunks_fts_insert AFTER INSERT ON content_chunks BEGIN
                INSERT INTO content_chunks_fts(rowid, normalized) VALUES (new.rowid, new.normalized);
            END;
            CREATE TRIGGER IF NOT EXISTS content_chunks_fts_delete AFTER DELETE ON content_chunks BEGIN
                INSERT INTO content_chunks_fts(content_chunks_fts, rowid, normalized) VALUES ('delete', old.rowid, old.normalized);
            END;
            COMMIT;
        ").map_err(storage)?;
        if version < 2 {
            // Databases created by version 1 have chunks but no search index yet. Fill the
            // index once from the stored chunks; the triggers keep it updated afterwards.
            connection
                .execute_batch("BEGIN IMMEDIATE; INSERT INTO content_chunks_fts(content_chunks_fts) VALUES('rebuild'); PRAGMA user_version=2; COMMIT;")
                .map_err(storage)?;
        }
        Ok(Self {
            connection: Mutex::new(connection),
            path: db_path.to_owned(),
            candidate_limit: MAX_CANDIDATES,
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
            if result.is_err() && !connection.is_autocommit() {
                // A cancelled operation can leave the transaction open: SQLite's full-text
                // index (FTS5) keeps an internal reader open, so the "interrupted" state stays
                // set and even the ROLLBACK fails with "interrupted". Closing the connection
                // always rolls the unfinished transaction back, so replace it with a new one.
                // Without this, every later call on this store would fail.
                *connection = connect(&self.path).map_err(storage)?;
            }
            if result.is_err() {
                check_cancel(cancel)?;
            }
            result
        })
    }

    // Unversioned fixture helper. Production publishes text and version together.
    #[cfg(test)]
    pub fn replace_document(
        &self,
        document: &ExtractedDocument,
        allowed_content_roots: &[PathBuf],
        cancel: &AtomicBool,
    ) -> Result<ContentDocumentSummary> {
        self.replace_document_versioned(document, allowed_content_roots, None, cancel)
    }
    /// Text and extraction identity are one commit. A user-unlocked PDF must
    /// never be published as an unversioned cache entry during reconciliation.
    pub(crate) fn replace_document_versioned(
        &self,
        document: &ExtractedDocument,
        allowed_content_roots: &[PathBuf],
        extractor_version: Option<&str>,
        cancel: &AtomicBool,
    ) -> Result<ContentDocumentSummary> {
        if extractor_version.is_some_and(|value| {
            value.is_empty() || value.len() > 128 || value.chars().any(char::is_control)
        }) {
            return Err(error(
                ContentStoreErrorCode::InvalidInput,
                "Invalid extraction version",
            ));
        }
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
            extractor_version: extractor_version.map(String::from),
        };
        self.run(cancel, |connection| {
            let tx = connection.transaction().map_err(storage)?;
            tx.execute("DELETE FROM content_documents WHERE path_key=?1", [&key]).map_err(storage)?;
            tx.execute("INSERT INTO content_documents(path_key,file_path,sha256,size_bytes,modified_unix_ms,format,section_count,chunk_count) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                params![key, source_path, document.fingerprint.sha256, document.fingerprint.size_bytes, document.fingerprint.modified_unix_ms, document.format, document.sections.len(), chunks.len()]).map_err(storage)?;
            let document_id = tx.last_insert_rowid();
            if let Some(version) = extractor_version {
                tx.execute("INSERT INTO content_extraction_versions(document_id,version) VALUES(?1,?2)", params![document_id, version]).map_err(storage)?;
            }
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

    /// Same as [`Self::search_detailed`] but returns only the hits. Kept so existing callers
    /// keep working; new code should use `search_detailed` to see partial-result warnings.
    pub fn search(
        &self,
        query: &str,
        filter: &ContentSearchFilter,
        allowed_content_roots: &[PathBuf],
        limit_count: usize,
        cancel: &AtomicBool,
    ) -> Result<Vec<ContentSearchHit>> {
        self.search_detailed(query, filter, allowed_content_roots, limit_count, cancel)
            .map(|response| response.hits)
    }

    pub fn search_detailed(
        &self,
        query: &str,
        filter: &ContentSearchFilter,
        allowed_content_roots: &[PathBuf],
        limit_count: usize,
        cancel: &AtomicBool,
    ) -> Result<ContentSearchResponse> {
        self.search_detailed_with_admission(
            query,
            filter,
            allowed_content_roots,
            limit_count,
            cancel,
            &mut |_| true,
        )
    }

    /// Narrow current source visibility before result slots and full chunks are
    /// selected. The callback cannot authorize a path outside the supplied roots.
    pub(crate) fn search_detailed_with_admission(
        &self,
        query: &str,
        filter: &ContentSearchFilter,
        allowed_content_roots: &[PathBuf],
        limit_count: usize,
        cancel: &AtomicBool,
        admit: &mut dyn FnMut(&Path) -> bool,
    ) -> Result<ContentSearchResponse> {
        check_cancel(cancel)?;
        let candidate_limit = self.candidate_limit;
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
        let extensions = normalized_extensions(&filter.extensions)?;
        let mut seen = HashSet::new();
        let terms: Vec<String> = query
            .split_whitespace()
            .map(normalized)
            .filter(|term| !term.is_empty() && seen.insert(term.clone()))
            .collect();
        if terms.is_empty() {
            return Ok(ContentSearchResponse::empty(self.candidate_limit));
        }
        if terms.len() > MAX_TERMS {
            return Err(error(
                ContentStoreErrorCode::InvalidInput,
                "Content search has too many terms",
            ));
        }
        let allowed = roots(allowed_content_roots);
        if allowed.is_empty() {
            return Ok(ContentSearchResponse::empty(self.candidate_limit));
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
            return Ok(ContentSearchResponse::empty(self.candidate_limit));
        }
        self.run(cancel, |connection| {
            let mut conditions = (1..=terms.len())
                .map(|n| format!("instr(c.normalized,?{n})>0"))
                .collect::<Vec<_>>();
            let mut arguments = terms.clone();
            // Restrict saved chat targets before applying the candidate budget.
            // Files still pass the canonical-root/fingerprint checks in step 2.
            if !documents.is_empty() {
                let mut slots=Vec::new();
                for path in &documents {
                    arguments.push(path.clone());
                    slots.push(format!("?{}",arguments.len()));
                }
                conditions.push(format!("d.path_key IN ({})",slots.join(",")));
            }
            // Extension filter: keep files whose path ends with ".<ext>" (any of them).
            // `lower()` is enough because extensions are ASCII-only (checked above).
            if !extensions.is_empty() {
                let mut options = Vec::new();
                for extension in &extensions {
                    arguments.push(format!("%.{extension}"));
                    options.push(format!("lower(d.file_path) LIKE ?{}", arguments.len()));
                }
                conditions.push(format!("({})", options.join(" OR ")));
            }
            // How candidates are found:
            // - The full-text index splits text into 3-character pieces ("trigrams"), so it can
            //   quickly find chunks containing any term of 3+ characters.
            // - It cannot look up 1-2 character terms, and we want results identical to the
            //   original substring search, so the `instr(...)` checks stay for every term.
            // The index only skips chunks that cannot match; it never changes which chunks match.
            let indexed: Vec<String> = terms
                .iter()
                .filter(|term| term.chars().count() >= 3)
                .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
                .collect();
            if !indexed.is_empty() {
                arguments.push(indexed.join(" AND "));
                conditions.insert(
                    0,
                    format!(
                        "c.rowid IN (SELECT rowid FROM content_chunks_fts WHERE content_chunks_fts MATCH ?{})",
                        arguments.len()
                    ),
                );
            }
            // Step 1 of 2: score every candidate cheaply.
            // Only small columns are read here (path, fingerprint, position and the
            // already-normalized text), not the full chunk. Checking file permissions is the
            // slow part (~0.7 ms per file), so it is postponed to step 2.
            let sql = format!(
                "SELECT c.rowid,d.file_path,d.sha256,d.size_bytes,d.modified_unix_ms,c.section_index,c.chunk_index,c.normalized                  FROM content_chunks c JOIN content_documents d ON d.id=c.document_id                  WHERE {} ORDER BY c.rowid LIMIT {}",
                conditions.join(" AND "),
                candidate_limit + 1
            );
            let mut statement = connection.prepare(&sql).map_err(storage)?;
            let mut rows = statement
                .query(params_from_iter(arguments.iter()))
                .map_err(storage)?;
            /// One candidate chunk with just enough data to rank it and check its file.
            struct Scored {
                rowid: i64,
                file_path: String,
                fingerprint: SourceFingerprint,
                section_index: u32,
                chunk_index: u32,
                score: f64,
            }
            let phrase = normalized(query);
            let mut scored = Vec::new();
            let mut hard_limit_reached = false;
            while let Some(row) = rows.next().map_err(storage)? {
                check_cancel(cancel)?;
                if scored.len() >= candidate_limit {
                    // A very common term matched too many chunks. Stop collecting, rank what
                    // we have, and tell the caller the result is partial.
                    hard_limit_reached = true;
                    break;
                }
                let searchable: String = row.get(7).map_err(storage)?;
                let score = terms
                    .iter()
                    .map(|term| searchable.match_indices(term).take(8).count() as f64 * 10.0)
                    .sum::<f64>()
                    + if searchable.contains(&phrase) {
                        25.0
                    } else {
                        0.0
                    };
                scored.push(Scored {
                    rowid: row.get(0).map_err(storage)?,
                    file_path: row.get(1).map_err(storage)?,
                    fingerprint: SourceFingerprint {
                        sha256: row.get(2).map_err(storage)?,
                        size_bytes: row.get(3).map_err(storage)?,
                        modified_unix_ms: row.get(4).map_err(storage)?,
                    },
                    section_index: row.get(5).map_err(storage)?,
                    chunk_index: row.get(6).map_err(storage)?,
                    score,
                });
            }
            drop(rows);
            drop(statement);
            let candidate_count = scored.len();
            scored.sort_by(|a, b| {
                b.score
                    .total_cmp(&a.score)
                    .then_with(|| a.file_path.cmp(&b.file_path))
                    .then_with(|| a.section_index.cmp(&b.section_index))
                    .then_with(|| a.chunk_index.cmp(&b.chunk_index))
            });
            // Step 2 of 2: walk candidates from best to worst and keep the visible ones.
            // Visibility is checked once per file (cached), and the full chunk is loaded only
            // for the hits we return. The result is the same as checking every candidate first,
            // because the order of visible candidates does not change.
            let mut visibility = HashMap::new();
            let mut fetch = connection
                .prepare(&format!("{CHUNK_SELECT} WHERE c.rowid=?1"))
                .map_err(storage)?;
            let mut hits = Vec::new();
            for candidate in scored {
                check_cancel(cancel)?;
                if hits.len() >= limit_count {
                    break;
                }
                let permitted = *visibility
                    .entry(candidate.file_path.clone())
                    .or_insert_with(|| {
                        let Some(path) =
                            visible(&candidate.file_path, &candidate.fingerprint, &allowed)
                        else {
                            return false;
                        };
                        (documents.is_empty() || documents.contains(&path_key(&path)))
                            && (scopes.is_empty()
                                || scopes.iter().any(|scope| within(&path, scope)))
                            && admit(&path)
                    });
                if !permitted {
                    continue;
                }
                let chunk = fetch
                    .query_row([candidate.rowid], read_chunk)
                    .map_err(storage)?;
                let snippet = plain_snippet(&chunk.text, &terms);
                hits.push(ContentSearchHit {
                    chunk,
                    score: candidate.score,
                    snippet,
                });
            }
            drop(fetch);
            check_cancel(cancel)?;
            // Check the returned files one more time: a file may have been edited or deleted
            // while we were ranking. Only the final hits (at most 100) are checked again.
            hits.retain(|hit| {
                visible(&hit.chunk.file_path, &hit.chunk.fingerprint, &allowed)
                    .is_some_and(|path| admit(&path))
            });
            Ok(ContentSearchResponse {
                hits,
                candidate_count,
                candidate_limit,
                hard_limit_reached,
            })
        })
    }

    /// Stream every matching file from one locked repository view. Unlike the
    /// ranked preview, this query has no chunk/result/candidate limit. EXISTS
    /// keeps section-level AND semantics and emits multi-chunk files only once.
    pub(crate) fn snapshot_keyword_files<E>(
        &self,
        query: &str,
        extensions: &[String],
        allowed_content_roots: &[PathBuf],
        cancel: &AtomicBool,
        visit: &mut dyn FnMut(&str) -> std::result::Result<(), E>,
    ) -> std::result::Result<(), SnapshotError<E>> {
        use SnapshotError::Source;
        if query.len() > MAX_QUERY_BYTES {
            return Err(Source(error(
                ContentStoreErrorCode::InvalidInput,
                "Search query is too long",
            )));
        }
        let extensions = normalized_extensions(extensions).map_err(Source)?;
        let mut seen = HashSet::new();
        let terms: Vec<_> = query
            .split_whitespace()
            .map(normalized)
            .filter(|term| !term.is_empty() && seen.insert(term.clone()))
            .collect();
        if terms.len() > MAX_TERMS {
            return Err(Source(error(
                ContentStoreErrorCode::InvalidInput,
                "Content search has too many terms",
            )));
        }
        let allowed = roots(allowed_content_roots);
        if terms.is_empty() || allowed.is_empty() {
            return check_cancel(cancel).map_err(Source);
        }
        let mut visitor_error = None;
        let result = self.run(cancel, |db| {
            let mut arguments = terms.clone();
            let mut chunk_conditions: Vec<_> = (1..=terms.len())
                .map(|n| format!("instr(c.normalized,?{n})>0")).collect();
            let indexed: Vec<_> = terms.iter().filter(|term| term.chars().count() >= 3)
                .map(|term| format!("\"{}\"", term.replace('"', "\"\""))).collect();
            if !indexed.is_empty() {
                arguments.push(indexed.join(" AND "));
                chunk_conditions.push(format!("c.rowid IN (SELECT rowid FROM content_chunks_fts WHERE content_chunks_fts MATCH ?{})", arguments.len()));
            }
            let mut conditions = vec![format!("EXISTS(SELECT 1 FROM content_chunks c WHERE c.document_id=d.id AND {})", chunk_conditions.join(" AND "))];
            if !extensions.is_empty() {
                let mut choices = Vec::new();
                for extension in &extensions {
                    arguments.push(format!("%.{extension}"));
                    choices.push(format!("lower(d.file_path) LIKE ?{}", arguments.len()));
                }
                conditions.push(format!("({})", choices.join(" OR ")));
            }
            let mut statement = db.prepare(&format!("SELECT d.file_path,d.sha256,d.size_bytes,d.modified_unix_ms FROM content_documents d WHERE {} ORDER BY d.path_key",conditions.join(" AND "))).map_err(storage)?;
            let mut rows = statement.query(params_from_iter(arguments.iter())).map_err(storage)?;
            while let Some(row) = rows.next().map_err(storage)? {
                check_cancel(cancel)?;
                let path: String = row.get(0).map_err(storage)?;
                let fingerprint = SourceFingerprint {
                    sha256: row.get(1).map_err(storage)?,
                    size_bytes: row.get(2).map_err(storage)?,
                    modified_unix_ms: row.get(3).map_err(storage)?,
                };
                if visible(&path, &fingerprint, &allowed).is_some() {
                    if let Err(error) = visit(&path) {
                        visitor_error = Some(error);
                        return Err(storage("Snapshot destination rejected a file"));
                    }
                }
            }
            check_cancel(cancel)
        });
        match visitor_error {
            Some(error) => Err(SnapshotError::Visitor(error)),
            None => result.map_err(Source),
        }
    }

    /// Keep known vector identities through uncertain source I/O. Search, RAG
    /// and writes must continue to use the current-source lookup below instead.
    pub(crate) fn retained_chunk_identities(
        &self,
        ids: &[String],
        allowed_content_roots: &[PathBuf],
        cancel: &AtomicBool,
    ) -> Result<Vec<CachedChunkIdentity>> {
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
            // Do not load excerpts, section headings or other document content.
            let mut statement = connection.prepare(
                "SELECT c.id,d.file_path,d.sha256,d.size_bytes,d.modified_unix_ms \
                 FROM content_chunks c JOIN content_documents d ON d.id=c.document_id \
                 JOIN content_sections s ON s.document_id=c.document_id AND s.section_index=c.section_index \
                 WHERE c.id=?1",
            ).map_err(storage)?;
            let mut output = Vec::new();
            let mut seen = HashSet::new();
            let mut files = HashMap::new();
            for id in ids {
                check_cancel(cancel)?;
                if !seen.insert(id) {
                    continue;
                }
                let identity = statement.query_row([id], |row| {
                    Ok(CachedChunkIdentity {
                        id: row.get(0)?,
                        file_path: row.get(1)?,
                        fingerprint: SourceFingerprint {
                            sha256: row.get(2)?,
                            size_bytes: row.get(3)?,
                            modified_unix_ms: row.get(4)?,
                        },
                    })
                }).optional().map_err(storage)?;
                if let Some(identity) = identity {
                    let keep = *files.entry(identity.file_path.clone()).or_insert_with(|| {
                        !cached_source_is_obsolete(&identity.file_path, &identity.fingerprint, &allowed)
                    });
                    if keep {
                        output.push(identity);
                    }
                }
            }
            check_cancel(cancel)?;
            Ok(output)
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

    /// Read the current source fingerprint and stored counts without loading
    /// sections or chunk text. Missing/changed/unauthorized sources are absent.
    pub fn document_summary(
        &self,
        path: &Path,
        allowed_content_roots: &[PathBuf],
        cancel: &AtomicBool,
    ) -> Result<Option<ContentDocumentSummary>> {
        check_cancel(cancel)?;
        let allowed = roots(allowed_content_roots);
        let Some(canonical) = safe_canonical(path, false) else {
            return Ok(None);
        };
        if !allowed.iter().any(|root| within(&canonical, root)) {
            return Ok(None);
        }
        let key = path_key(&canonical);
        self.run(cancel, |connection| {
            let summary = connection
                .query_row(
                    &format!("{DOCUMENT_SELECT} WHERE path_key=?1"),
                    [key],
                    read_summary,
                )
                .optional()
                .map_err(storage)?;
            Ok(summary.filter(|summary| {
                visible(&summary.file_path, &summary.fingerprint, &allowed).is_some()
            }))
        })
    }

    /// Stamp only a successfully replaced, still-current source. An interrupted
    /// parser upgrade leaves older documents unstamped and eligible for retry.
    #[cfg(test)]
    pub fn mark_extraction_version(
        &self,
        document: &ContentDocumentSummary,
        version: &str,
        allowed_roots: &[PathBuf],
        cancel: &AtomicBool,
    ) -> Result<()> {
        if version.is_empty() || version.len() > 128 || version.chars().any(char::is_control) {
            return Err(error(
                ContentStoreErrorCode::InvalidInput,
                "Invalid extraction pipeline version",
            ));
        }
        let current =
            self.document_summary(Path::new(&document.file_path), allowed_roots, cancel)?;
        if !current.is_some_and(|current| current.fingerprint == document.fingerprint) {
            return Err(error(
                ContentStoreErrorCode::SourceChanged,
                "Source changed before extraction version commit",
            ));
        }
        let key = path_key(Path::new(&document.file_path));
        self.run(cancel, |connection| {
            let changed = connection.execute("INSERT INTO content_extraction_versions(document_id,version) SELECT id,?1 FROM content_documents WHERE path_key=?2 AND sha256=?3 ON CONFLICT(document_id) DO UPDATE SET version=excluded.version", params![version,key,document.fingerprint.sha256]).map_err(storage)?;
            if changed != 1 { return Err(error(ContentStoreErrorCode::SourceChanged, "Source changed before extraction version commit")); }
            Ok(())
        })
    }

    /// Enumerate visible document metadata only; a budget overflow is an error,
    /// never a silently truncated project/status listing.
    pub fn documents(
        &self,
        allowed_content_roots: &[PathBuf],
        cancel: &AtomicBool,
    ) -> Result<Vec<ContentDocumentSummary>> {
        check_cancel(cancel)?;
        let allowed = roots(allowed_content_roots);
        if allowed.is_empty() {
            return Ok(Vec::new());
        }
        self.run(cancel, |connection| {
            let mut statement = connection
                .prepare(&format!("{DOCUMENT_SELECT} ORDER BY file_path"))
                .map_err(storage)?;
            let mut rows = statement.query([]).map_err(storage)?;
            let mut summaries = Vec::new();
            while let Some(row) = rows.next().map_err(storage)? {
                check_cancel(cancel)?;
                let summary = read_summary(row).map_err(storage)?;
                if visible(&summary.file_path, &summary.fingerprint, &allowed).is_some() {
                    if summaries.len() == 100_000 {
                        return Err(limit());
                    }
                    summaries.push(summary);
                }
            }
            check_cancel(cancel)?;
            Ok(summaries)
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
        self.purge_for_scopes(allowed_content_roots, &[], cancel)
    }
    /// Keep unavailable configured folders cached, without making their text visible.
    /// Removed folders are deliberately absent from `unavailable_roots`.
    pub fn purge_for_scopes(
        &self,
        allowed_content_roots: &[PathBuf],
        unavailable_roots: &[PathBuf],
        cancel: &AtomicBool,
    ) -> Result<usize> {
        let allowed = roots(allowed_content_roots);
        let unavailable: Vec<PathBuf> = unavailable_roots
            .iter()
            // An offline volume may report device-not-ready before we can
            // expand aliases. Preserve its configured cache lexically; this
            // branch never authorizes source reads.
            .map(|root| {
                purge_path(root)
                    .or_else(|_| std::path::absolute(normal_path(root)).map_err(storage))
            })
            .collect::<Result<_>>()?;
        self.purge_where(cancel, |path, fingerprint| {
            !unavailable
                .iter()
                .any(|root| within(&normal_path(Path::new(path)), root))
                && cached_source_is_obsolete(path, fingerprint, &allowed)
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
const DOCUMENT_SELECT: &str = "SELECT d.file_path,d.sha256,d.size_bytes,d.modified_unix_ms,d.format,d.section_count,d.chunk_count,v.version FROM content_documents d LEFT JOIN content_extraction_versions v ON v.document_id=d.id";
fn read_summary(row: &rusqlite::Row<'_>) -> rusqlite::Result<ContentDocumentSummary> {
    Ok(ContentDocumentSummary {
        file_path: row.get(0)?,
        fingerprint: SourceFingerprint {
            sha256: row.get(1)?,
            size_bytes: row.get(2)?,
            modified_unix_ms: row.get(3)?,
        },
        format: row.get(4)?,
        section_count: row.get(5)?,
        chunk_count: row.get(6)?,
        extractor_version: row.get(7)?,
    })
}
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

#[cfg(all(test, windows))]
pub(crate) mod transient_source_fixture {
    use super::*;
    use std::{
        io,
        os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
    };
    use windows_sys::Win32::Storage::FileSystem::{
        FileDispositionInfo, SetFileInformationByHandle, DELETE, FILE_DISPOSITION_INFO,
    };
    // A share-denied data handle still permits metadata queries. Pending
    // deletion gives a real, reversible access-denied error without ACL
    // changes. This fixture operates only on its own temporary files.
    pub(crate) struct Pending(fs::File);
    impl Pending {
        pub(crate) fn set(&self, value: bool) -> io::Result<()> {
            let info = FILE_DISPOSITION_INFO { DeleteFile: value };
            let ok = unsafe {
                SetFileInformationByHandle(
                    self.0.as_raw_handle() as _,
                    FileDispositionInfo,
                    (&info as *const FILE_DISPOSITION_INFO).cast(),
                    std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
                )
            };
            if ok == 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        }
        pub(crate) fn new(path: &Path) -> Self {
            let pending = Self(
                fs::OpenOptions::new()
                    .access_mode(DELETE)
                    .share_mode(7)
                    .open(path)
                    .unwrap(),
            );
            pending.set(true).unwrap();
            pending
        }
    }
    impl Drop for Pending {
        fn drop(&mut self) {
            let _ = self.set(false);
        }
    }
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
    fn admission_backfills_result_slots_and_rechecks_before_returning_cached_text() {
        let directory = tempfile::tempdir().unwrap();
        let roots = vec![directory.path().join("sources")];
        let store = ContentStore::open(&directory.path().join("index.sqlite")).unwrap();
        for name in ["a-denied.md", "b-denied.md", "z-allowed.md"] {
            let doc = document(&roots[0], name, "admissioncanary evidence");
            store.replace_document(&doc, &roots, &cancel()).unwrap();
        }
        let filter = ContentSearchFilter::default();
        let baseline = store
            .search_detailed("admissioncanary", &filter, &roots, 1, &cancel())
            .unwrap();
        assert!(baseline.hits[0].chunk.file_path.ends_with("a-denied.md"));
        let admitted = store
            .search_detailed_with_admission(
                "admissioncanary",
                &filter,
                &roots,
                1,
                &cancel(),
                &mut |path| path.ends_with("z-allowed.md"),
            )
            .unwrap();
        assert_eq!(admitted.hits.len(), 1);
        assert!(admitted.hits[0].chunk.file_path.ends_with("z-allowed.md"));
        assert_eq!(admitted.candidate_count, baseline.candidate_count);
        assert_eq!(admitted.candidate_limit, baseline.candidate_limit);

        let mut checks = 0;
        let revoked = store
            .search_detailed_with_admission(
                "admissioncanary",
                &filter,
                &roots,
                1,
                &cancel(),
                &mut |_| {
                    checks += 1;
                    checks == 1
                },
            )
            .unwrap();
        assert!(revoked.hits.is_empty());
        assert_eq!(checks, 2);
        let outside = store
            .search_detailed_with_admission(
                "admissioncanary",
                &filter,
                &[],
                1,
                &cancel(),
                &mut |_| true,
            )
            .unwrap();
        assert!(outside.hits.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn unix_scopes_extract_search_and_purge_without_allowing_child_links() {
        // /tmp is a real directory on Linux and the actual system alias on
        // macOS. Compile and exercise the rest of the contract on both hosts.
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let roots = vec![dir.path().to_owned()];
        let doc = document(dir.path(), "source.md", "한국어 alias evidence");
        let canonical = fs::canonicalize(dir.path()).unwrap();
        assert_eq!(Path::new(&doc.file_path), canonical.join("source.md"));
        let store = ContentStore::open(&dir.path().join("index.sqlite")).unwrap();
        store.replace_document(&doc, &roots, &cancel()).unwrap();
        assert_eq!(search(&store, "evidence", &roots).len(), 1);
        assert_eq!(
            search(&store, "evidence", std::slice::from_ref(&canonical)).len(),
            1
        );
        let linked = dir.path().join("linked");
        std::os::unix::fs::symlink(&canonical, &linked).unwrap();
        assert!(search(&store, "evidence", std::slice::from_ref(&linked)).is_empty());
        let error = extract_document(
            &linked.join("source.md"),
            &ExtractOptions::default(),
            &cancel(),
        )
        .unwrap_err();
        assert_eq!(
            error.code,
            crate::document_extract::ExtractErrorCode::UnsafePath
        );
        fs::remove_file(dir.path().join("source.md")).unwrap();
        assert_eq!(
            store
                .purge_document(&dir.path().join("source.md"), &cancel())
                .unwrap(),
            1
        );
        assert!(search(&store, "evidence", &roots).is_empty());
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
    fn extraction_version_and_unlocked_text_commit_or_roll_back_together() {
        let dir = tempfile::tempdir().unwrap();
        let roots = vec![dir.path().to_owned()];
        let db = dir.path().join("index.sqlite");
        let store = ContentStore::open(&db).unwrap();
        let mut doc = document(dir.path(), "source.txt", "previouscanary");
        store
            .replace_document_versioned(&doc, &roots, Some("previous"), &cancel())
            .unwrap();
        store
            .connection
            .lock()
            .unwrap()
            .execute_batch(
                "CREATE TRIGGER reject_version BEFORE INSERT ON content_extraction_versions
             WHEN new.version='rejected' BEGIN SELECT RAISE(ABORT,'fixture failure'); END;",
            )
            .unwrap();
        doc.sections[0].text = "replacementcanary".into();
        assert!(store
            .replace_document_versioned(&doc, &roots, Some("rejected"), &cancel())
            .is_err());
        assert_eq!(search(&store, "previouscanary", &roots).len(), 1);
        assert!(search(&store, "replacementcanary", &roots).is_empty());
        assert_eq!(
            store
                .document_summary(Path::new(&doc.file_path), &roots, &cancel())
                .unwrap()
                .unwrap()
                .extractor_version
                .as_deref(),
            Some("previous")
        );
        let summary = store
            .replace_document_versioned(&doc, &roots, Some("accepted"), &cancel())
            .unwrap();
        assert_eq!(summary.extractor_version.as_deref(), Some("accepted"));
        drop(store);
        let reopened = ContentStore::open(&db).unwrap();
        assert_eq!(search(&reopened, "replacementcanary", &roots).len(), 1);
        assert!(search(&reopened, "previouscanary", &roots).is_empty());
        assert_eq!(
            reopened
                .document_summary(Path::new(&doc.file_path), &roots, &cancel())
                .unwrap()
                .unwrap()
                .extractor_version
                .as_deref(),
            Some("accepted")
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
            ..Default::default()
        };
        assert!(store
            .search("shared", &filter, &[a.clone()], 20, &cancel())
            .unwrap()
            .is_empty());
        let filter = ContentSearchFilter {
            document_paths: vec![],
            scope_paths: vec![a.to_string_lossy().into_owned()],
            ..Default::default()
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
            ..Default::default()
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
    fn unavailable_configured_folders_keep_cache_without_authorizing_search() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("removable");
        let doc = document(&folder, "source.txt", "reconnectable source");
        let store = ContentStore::open(&dir.path().join("index.sqlite")).unwrap();
        store
            .replace_document(&doc, &[folder.clone()], &cancel())
            .unwrap();
        assert_eq!(
            store
                .purge_for_scopes(&[], &[folder.clone()], &cancel())
                .unwrap(),
            0
        );
        assert!(search(&store, "reconnectable", &[]).is_empty());
        assert_eq!(search(&store, "reconnectable", &[folder]).len(), 1);
        assert_eq!(store.purge_for_scopes(&[], &[], &cancel()).unwrap(), 1);
    }
    #[cfg(windows)]
    #[test]
    fn transient_source_failures_hide_text_without_losing_cached_extraction() {
        use super::transient_source_fixture::Pending;
        use std::io;
        let dir = tempfile::tempdir().unwrap();
        let roots = vec![dir.path().to_owned()];
        let doc = document(dir.path(), "source.txt", "reusable cached evidence");
        let store = ContentStore::open(&dir.path().join("index.sqlite")).unwrap();
        store.replace_document(&doc, &roots, &cancel()).unwrap();
        let source = Path::new(&doc.file_path);
        let ids: Vec<_> = store
            .chunks_for_document(source, &roots, &cancel())
            .unwrap()
            .iter()
            .map(|chunk| chunk.id.clone())
            .collect();
        let held = Pending::new(source);
        assert_eq!(
            fs::canonicalize(source).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        assert!(search(&store, "evidence", &roots).is_empty());
        assert!(store
            .chunks_by_ids(&ids, &roots, &cancel())
            .unwrap()
            .is_empty());
        assert_eq!(
            store
                .retained_chunk_identities(&ids, &roots, &cancel())
                .unwrap()
                .len(),
            ids.len()
        );
        assert!(store
            .retained_chunk_identities(&ids, &[], &cancel())
            .unwrap()
            .is_empty());
        assert_eq!(store.purge_outside_roots(&roots, &cancel()).unwrap(), 0);
        assert_eq!(
            store
                .connection
                .lock()
                .unwrap()
                .query_row("SELECT count(*) FROM content_documents", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        held.set(false).unwrap();
        drop(held);
        assert_eq!(search(&store, "evidence", &roots).len(), 1);
        fs::remove_file(source).unwrap();
        assert!(store
            .retained_chunk_identities(&ids, &roots, &cancel())
            .unwrap()
            .is_empty());
        assert_eq!(store.purge_outside_roots(&roots, &cancel()).unwrap(), 1);

        let doc = document(dir.path(), "revoked.txt", "revoked cached evidence");
        store.replace_document(&doc, &roots, &cancel()).unwrap();
        let held = Pending::new(Path::new(&doc.file_path));
        // Uncertain source I/O must never postpone an actual scope revocation.
        assert_eq!(store.purge_outside_roots(&[], &cancel()).unwrap(), 1);
        held.set(false).unwrap();
        drop(held);
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
    #[cfg(windows)]
    #[test]
    fn real_short_paths_purge_files_and_scopes_without_touching_neighbors() {
        let directory = tempfile::tempdir().unwrap();
        let folder = directory.path().join("A long indexed document folder");
        fs::create_dir(&folder).unwrap();
        let Some(short) = crate::path_identity::short_alias(&folder).unwrap() else {
            eprintln!("No short aliases on fixture volume; actual alias case not exercised");
            return;
        };
        let roots = vec![directory.path().to_owned()];
        let store = ContentStore::open(&directory.path().join("index.sqlite")).unwrap();
        for (parent, name) in [
            (&folder, "a.md"),
            (&folder, "sub/b.md"),
            (
                &directory
                    .path()
                    .join("A long indexed document folder-other"),
                "other.md",
            ),
        ] {
            let doc = document(parent, name, "shared");
            store.replace_document(&doc, &roots, &cancel()).unwrap();
        }
        fs::remove_file(folder.join("a.md")).unwrap();
        assert_eq!(
            store
                .purge_document(&short.join("a.md"), &cancel())
                .unwrap(),
            1
        );
        assert_eq!(store.purge_scope(&short, &cancel()).unwrap(), 1);
        assert_eq!(search(&store, "shared", &roots).len(), 1);
        assert!(search(&store, "shared", &roots)[0]
            .chunk
            .file_path
            .ends_with("other.md"));
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
    fn document_metadata_reads_exclude_changed_missing_and_unauthorized_sources() {
        let dir = tempfile::tempdir().unwrap();
        let inside = dir.path().join("inside");
        let outside = dir.path().join("outside");
        let all = vec![dir.path().to_owned()];
        let allowed = vec![inside.clone()];
        let store = ContentStore::open(&dir.path().join("index.sqlite")).unwrap();
        let first = document(&inside, "first.txt", "first source\nsecond section");
        let empty = document(&inside, "empty.txt", "");
        let other = document(&outside, "other.txt", "outside source");
        for doc in [&first, &empty, &other] {
            store.replace_document(doc, &all, &cancel()).unwrap();
        }
        let summary = store
            .document_summary(Path::new(&first.file_path), &allowed, &cancel())
            .unwrap()
            .unwrap();
        assert_eq!(summary.fingerprint, first.fingerprint);
        assert_eq!(summary.section_count, 2);
        assert_eq!(summary.chunk_count, 2);
        let summaries = store.documents(&allowed, &cancel()).unwrap();
        assert_eq!(summaries.len(), 2);
        assert!(summaries
            .iter()
            .any(|summary| summary.chunk_count == 0 && summary.fingerprint == empty.fingerprint));
        assert!(store
            .document_summary(Path::new(&other.file_path), &allowed, &cancel())
            .unwrap()
            .is_none());
        assert!(store
            .document_summary(&inside.join("missing.txt"), &allowed, &cancel())
            .unwrap()
            .is_none());
        assert!(store.documents(&[], &cancel()).unwrap().is_empty());
        fs::write(&first.file_path, "changed source with a new byte count").unwrap();
        assert!(store
            .document_summary(Path::new(&first.file_path), &allowed, &cancel())
            .unwrap()
            .is_none());
        fs::remove_file(&empty.file_path).unwrap();
        assert!(store.documents(&allowed, &cancel()).unwrap().is_empty());
        assert_eq!(
            store
                .documents(&all, &AtomicBool::new(true))
                .unwrap_err()
                .code,
            ContentStoreErrorCode::Cancelled
        );
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
    fn trigram_index_matches_linear_semantics_and_tracks_changes() {
        let dir = tempfile::tempdir().unwrap();
        let roots = vec![dir.path().to_owned()];
        let db = dir.path().join("index.sqlite");
        let store = ContentStore::open(&db).unwrap();
        let doc = document(dir.path(), "a.txt", "한 국 어 예산 Budget\"Quote\" 100%_x");
        store.replace_document(&doc, &roots, &cancel()).unwrap();
        // 3+ character terms go through FTS, shorter ones through instr; both still AND.
        assert_eq!(search(&store, "한국어 예산", &roots).len(), 1);
        assert_eq!(search(&store, "budget\"quote\"", &roots).len(), 1);
        assert_eq!(search(&store, "100%_x 예", &roots).len(), 1);
        assert!(search(&store, "한국어 없음", &roots).is_empty());
        // Replacing and purging keep the trigram index in sync (no stale candidates).
        let doc = document(dir.path(), "a.txt", "replacement words");
        store.replace_document(&doc, &roots, &cancel()).unwrap();
        assert!(search(&store, "한국어", &roots).is_empty());
        assert_eq!(search(&store, "replacement", &roots).len(), 1);
        let count: i64 = store
            .connection
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM content_chunks_fts WHERE content_chunks_fts MATCH '\"한국어\"'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
        drop(store);
        // A v1 store (no FTS) is upgraded and its existing chunks become searchable.
        let connection = Connection::open(&db).unwrap();
        connection
            .execute_batch("DROP TRIGGER content_chunks_fts_insert; DROP TRIGGER content_chunks_fts_delete; DROP TABLE content_chunks_fts; PRAGMA user_version=1;")
            .unwrap();
        drop(connection);
        let store = ContentStore::open(&db).unwrap();
        assert_eq!(search(&store, "replacement", &roots).len(), 1);
        drop(store);
        let connection = Connection::open(&db).unwrap();
        connection.execute_batch("PRAGMA user_version=3;").unwrap();
        drop(connection);
        assert!(ContentStore::open(&db).is_err());
    }

    #[test]
    fn detailed_search_reports_partial_results_at_the_candidate_limit() {
        let dir = tempfile::tempdir().unwrap();
        let inside = dir.path().join("inside");
        let roots = vec![inside.clone()];
        let wide = vec![dir.path().to_owned()];
        let mut store = ContentStore::open(&dir.path().join("index.sqlite")).unwrap();
        // Rowid order: three documents outside the requested scope come first.
        for name in ["a.txt", "b.txt", "c.txt"] {
            let doc = document(dir.path(), name, "공통단어 outside");
            store.replace_document(&doc, &wide, &cancel()).unwrap();
        }
        let doc = document(&inside, "d.txt", "공통단어 inside");
        store.replace_document(&doc, &wide, &cancel()).unwrap();
        let detailed = |store: &ContentStore, roots: &[PathBuf]| {
            store
                .search_detailed(
                    "공통단어",
                    &ContentSearchFilter::default(),
                    roots,
                    20,
                    &cancel(),
                )
                .unwrap()
        };
        let full = detailed(&store, &roots);
        assert_eq!(
            (
                full.hits.len(),
                full.candidate_count,
                full.hard_limit_reached
            ),
            (1, 4, false)
        );
        assert_eq!(full.candidate_limit, MAX_CANDIDATES);
        store.candidate_limit = 3;
        // Only out-of-scope candidates fit the budget: zero hits must be reported as partial.
        let partial = detailed(&store, &roots);
        assert!(partial.hits.is_empty());
        assert_eq!((partial.candidate_count, partial.candidate_limit), (3, 3));
        assert!(partial.hard_limit_reached);
        let json = serde_json::to_value(&partial).unwrap();
        assert_eq!(json["hardLimitReached"], true);
        assert_eq!(json["candidateCount"], 3);
        // The compatibility wrapper returns the same hits.
        assert!(search(&store, "공통단어", &roots).is_empty());
        let selected = store
            .search_detailed(
                "공통단어",
                &ContentSearchFilter {
                    document_paths: vec![doc.file_path.clone()],
                    ..Default::default()
                },
                &roots,
                20,
                &cancel(),
            )
            .unwrap();
        assert_eq!(
            selected.hits.len(),
            1,
            "Document filters run before unrelated rows exhaust the budget"
        );
        assert_eq!(selected.candidate_count, 1);
        assert!(!selected.hard_limit_reached);
        store.candidate_limit = MAX_CANDIDATES;
        assert_eq!(search(&store, "공통단어", &roots).len(), 1);
        assert_eq!(detailed(&store, &roots).candidate_count, 4);
        assert!(!detailed(&store, &[]).hard_limit_reached);
    }

    #[test]
    fn extraction_versions_survive_restart_and_reset_on_projection_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let roots = vec![dir.path().to_owned()];
        let path = dir.path().join("index.sqlite");
        let store = ContentStore::open(&path).unwrap();
        let mut doc = document(dir.path(), "projection.txt", "projection fixture");
        let summary = store.replace_document(&doc, &roots, &cancel()).unwrap();
        assert!(summary.extractor_version.is_none());
        store
            .mark_extraction_version(&summary, "parser-v2", &roots, &cancel())
            .unwrap();
        drop(store);
        let store = ContentStore::open(&path).unwrap();
        assert_eq!(
            store
                .document_summary(Path::new(&doc.file_path), &roots, &cancel())
                .unwrap()
                .unwrap()
                .extractor_version
                .as_deref(),
            Some("parser-v2")
        );
        doc.sections[0].text.push_str(" new derived projection");
        store.replace_document(&doc, &roots, &cancel()).unwrap();
        assert!(store
            .document_summary(Path::new(&doc.file_path), &roots, &cancel())
            .unwrap()
            .unwrap()
            .extractor_version
            .is_none());
        assert!(store
            .mark_extraction_version(&summary, "parser-v3", &[], &cancel())
            .is_err());
    }

    #[test]
    fn extension_filter_has_no_file_count_limit_and_ignores_dots_and_case() {
        let dir = tempfile::tempdir().unwrap();
        let roots = vec![dir.path().to_owned()];
        let store = ContentStore::open(&dir.path().join("index.sqlite")).unwrap();
        // More files than the 256-entry document_paths budget, to show there is no such cap.
        for i in 0..300 {
            let doc = document(dir.path(), &format!("note{i}.txt"), "확장자검색 본문");
            store.replace_document(&doc, &roots, &cancel()).unwrap();
        }
        let doc = document(dir.path(), "Guide.MD", "확장자검색 본문");
        store.replace_document(&doc, &roots, &cancel()).unwrap();
        let only = |extensions: &[&str]| {
            let filter = ContentSearchFilter {
                extensions: extensions.iter().map(|e| e.to_string()).collect(),
                ..Default::default()
            };
            store.search_detailed("확장자검색", &filter, &roots, 100, &cancel())
        };
        let text = only(&["TXT"]).unwrap();
        assert_eq!(text.hits.len(), 100); // limited by the result count, not the file count
        assert_eq!(text.candidate_count, 300);
        assert!(text
            .hits
            .iter()
            .all(|hit| hit.chunk.file_path.ends_with(".txt")));
        let markdown = only(&[".md"]).unwrap();
        assert_eq!(markdown.hits.len(), 1);
        assert!(markdown.hits[0].chunk.file_path.ends_with("Guide.MD"));
        assert_eq!(only(&["md", "txt", ".MD"]).unwrap().candidate_count, 301);
        assert!(only(&["pdf"]).unwrap().hits.is_empty());
        assert_eq!(only(&[]).unwrap().candidate_count, 301);
        // Wildcards, paths, empty values and too many filters are rejected.
        for bad in [&["%"][..], &["t_t"], &["a/b"], &[""], &["."], &["한글"]] {
            assert_eq!(
                only(bad).unwrap_err().code,
                ContentStoreErrorCode::InvalidInput
            );
        }
        let many: Vec<String> = (0..17).map(|i| format!("e{i}")).collect();
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        assert_eq!(
            only(&many).unwrap_err().code,
            ContentStoreErrorCode::InvalidInput
        );
        // Old clients that do not send `extensions` still deserialize.
        let old: ContentSearchFilter =
            serde_json::from_str(r#"{"documentPaths":[],"scopePaths":[]}"#).unwrap();
        assert!(old.extensions.is_empty());
    }

    #[test]
    fn final_source_check_drops_hits_changed_after_ranking() {
        // The final recheck uses the same visibility rule as ranking; a changed source vanishes.
        let dir = tempfile::tempdir().unwrap();
        let roots = vec![dir.path().to_owned()];
        let store = ContentStore::open(&dir.path().join("index.sqlite")).unwrap();
        let doc = document(dir.path(), "x.txt", "검사대상 내용");
        store.replace_document(&doc, &roots, &cancel()).unwrap();
        assert_eq!(search(&store, "검사대상", &roots).len(), 1);
        fs::write(dir.path().join("x.txt"), "검사대상 내용이 바뀜").unwrap();
        assert!(search(&store, "검사대상", &roots).is_empty());
    }

    #[test]
    fn keyword_snapshot_streams_past_preview_budgets_with_current_visibility() {
        let directory = tempfile::tempdir().unwrap();
        let permitted = directory.path().join("allowed");
        fs::create_dir(&permitted).unwrap();
        let roots = vec![permitted.clone()];
        let mut store = ContentStore::open(&directory.path().join("index.sqlite")).unwrap();
        store.candidate_limit = 3;
        for index in 0..107 {
            let doc = document(
                &permitted,
                &format!("{index:03}.MD"),
                &"한국어 snapshotneedle 공용 ".repeat(150),
            );
            store.replace_document(&doc, &roots, &cancel()).unwrap();
        }
        let other = document(directory.path(), "outside.md", "한국어 snapshotneedle 공용");
        store
            .replace_document(&other, &[directory.path().to_owned()], &cancel())
            .unwrap();
        let split = document(&permitted, "split.md", "snapshotneedle\n공용");
        store.replace_document(&split, &roots, &cancel()).unwrap();
        let changed = document(&permitted, "changed.md", "snapshotneedle 공용");
        store.replace_document(&changed, &roots, &cancel()).unwrap();
        fs::write(permitted.join("changed.md"), "changed content").unwrap();
        let preview = store
            .search_detailed(
                "snapshotneedle 공용",
                &ContentSearchFilter::default(),
                &roots,
                100,
                &cancel(),
            )
            .unwrap();
        assert!(preview.hard_limit_reached);
        let mut paths = Vec::new();
        store
            .snapshot_keyword_files(
                "snapshotneedle 공용",
                &[".md".into()],
                &roots,
                &cancel(),
                &mut |path| {
                    paths.push(path.to_owned());
                    Ok::<_, ()>(())
                },
            )
            .unwrap();
        assert_eq!(paths.len(), 107);
        assert!(paths.iter().any(|path| path.ends_with("106.MD")));
        assert!(!paths.iter().any(|path| path.ends_with("split.md")
            || path.ends_with("outside.md")
            || path.ends_with("changed.md")));
        let mut short = Vec::new();
        store
            .snapshot_keyword_files("공용", &[], &roots, &cancel(), &mut |path| {
                short.push(path.to_owned());
                Ok::<_, ()>(())
            })
            .unwrap();
        assert_eq!(short.len(), 108); // short-term fallback includes split.md
    }
    #[test]
    fn keyword_snapshot_preserves_destination_errors_and_cancels_without_poisoning_reads() {
        let directory = tempfile::tempdir().unwrap();
        let roots = vec![directory.path().to_owned()];
        let store = ContentStore::open(&directory.path().join("index.sqlite")).unwrap();
        for name in ["a.md", "b.md"] {
            let doc = document(directory.path(), name, "snapshotneedle");
            store.replace_document(&doc, &roots, &cancel()).unwrap();
        }
        let failed =
            store.snapshot_keyword_files("snapshotneedle", &[], &roots, &cancel(), &mut |_| {
                Err("destination-full")
            });
        assert!(matches!(
            failed,
            Err(SnapshotError::Visitor("destination-full"))
        ));
        let stop = cancel();
        let mut visited = 0;
        let cancelled =
            store.snapshot_keyword_files("snapshotneedle", &[], &roots, &stop, &mut |_| {
                visited += 1;
                stop.store(true, Ordering::Release);
                Ok::<_, ()>(())
            });
        assert!(matches!(
            cancelled,
            Err(SnapshotError::Source(ContentStoreError {
                code: ContentStoreErrorCode::Cancelled,
                ..
            }))
        ));
        assert_eq!(visited, 1);
        assert_eq!(search(&store, "snapshotneedle", &roots).len(), 2);
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
