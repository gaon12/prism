//! Persistent gallery hierarchy. These operations change only library metadata;
//! removing a collection, work, unit or source never deletes an original file.
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ErrorCode {
    Input,
    NotFound,
    Conflict,
    Storage,
    SourceChanged,
    Cancelled,
    Unsupported,
    Limit,
    UnsafePath,
    Busy,
    Encrypted,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn setup() -> (tempfile::TempDir, CollectionStore, Collection, Work, Unit) {
        let directory = tempfile::tempdir().unwrap();
        let store = CollectionStore::open(&directory.path().join("library")).unwrap();
        let collection = store.create_collection("만화와 문서", "원본 참조").unwrap();
        let work = store
            .create_work(
                &collection.id,
                "작품",
                WorkMetadata {
                    kind: WorkKind::Comic,
                    direction: ReadingDirection::RightToLeft,
                    ..Default::default()
                },
            )
            .unwrap();
        let unit = store
            .create_unit(&work.id, "1권", UnitKind::Volume)
            .unwrap();
        (directory, store, collection, work, unit)
    }
    fn page(unit: &Unit, source: &Source, n: u32) -> Page {
        Page {
            id: format!("{n:064x}"),
            unit_id: unit.id.clone(),
            source_id: source.id.clone(),
            position: 999,
            title: format!("{n}페이지"),
            location: PageLocation {
                kind: PageKind::Image,
                entry: None,
                number: n,
            },
            source_hash: "a".repeat(64),
        }
    }
    #[test]
    fn hierarchy_reopens_preserves_reading_and_removes_only_references() {
        let (dir, store, collection, work, unit) = setup();
        let original = dir.path().join("원본.png");
        fs::write(&original, b"keep original").unwrap();
        let source = store
            .add_source(&unit.id, &original, SourceKind::Image)
            .unwrap();
        let unit = store.unit(&unit.id).unwrap();
        let pages = vec![page(&unit, &source, 1), page(&unit, &source, 2)];
        let unit = store
            .replace_pages(
                &unit.id,
                unit.revision,
                pages.clone(),
                &AtomicBool::new(false),
            )
            .unwrap();
        store
            .save_reading(&work.id, Some(&pages[1].id), false)
            .unwrap();
        let reopened = CollectionStore::open(&dir.path().join("library")).unwrap();
        assert_eq!(
            reopened.reading(&work.id).unwrap().page_id.as_deref(),
            Some(pages[1].id.as_str())
        );
        assert_eq!(reopened.work(&work.id).unwrap().page_count, 2);
        assert_eq!(
            reopened.first_page(&work.id).unwrap().unwrap().id,
            pages[0].id
        );
        assert_eq!(
            reopened
                .pages(&unit.id, unit.revision, 0, 1)
                .unwrap()
                .next_offset,
            Some(1)
        );
        reopened
            .remove_collection(&collection.id, collection.revision)
            .unwrap();
        assert_eq!(
            reopened.work(&work.id).unwrap_err().code,
            ErrorCode::NotFound
        );
        assert_eq!(
            reopened.source(&source.id).unwrap_err().code,
            ErrorCode::NotFound
        );
        assert_eq!(fs::read(original).unwrap(), b"keep original");
    }
    #[test]
    fn cancelled_invalid_and_stale_manifests_keep_prior_pages_and_bookmark() {
        let (dir, store, _, work, unit) = setup();
        let source = store
            .add_source(&unit.id, &dir.path().join("page.png"), SourceKind::Image)
            .unwrap();
        let unit = store.unit(&unit.id).unwrap();
        let old = page(&unit, &source, 1);
        let unit = store
            .replace_pages(
                &unit.id,
                unit.revision,
                [old.clone()],
                &AtomicBool::new(false),
            )
            .unwrap();
        store.save_reading(&work.id, Some(&old.id), false).unwrap();
        let cancel = AtomicBool::new(false);
        let incoming = [page(&unit, &source, 2), page(&unit, &source, 3)]
            .into_iter()
            .inspect(|p| {
                if p.location.number == 3 {
                    cancel.store(true, Ordering::Release);
                }
            });
        assert_eq!(
            store
                .replace_pages(&unit.id, unit.revision, incoming, &cancel)
                .unwrap_err()
                .code,
            ErrorCode::Cancelled
        );
        let mut bad = page(&unit, &source, 4);
        bad.source_id = "f".repeat(32);
        assert_eq!(
            store
                .replace_pages(&unit.id, unit.revision, [bad], &AtomicBool::new(false))
                .unwrap_err()
                .code,
            ErrorCode::Input
        );
        assert_eq!(
            store
                .replace_pages(
                    &unit.id,
                    unit.revision - 1,
                    [page(&unit, &source, 4)],
                    &AtomicBool::new(false)
                )
                .unwrap_err()
                .code,
            ErrorCode::Conflict
        );
        assert_eq!(
            store.pages(&unit.id, unit.revision, 0, 10).unwrap().items[0].id,
            old.id
        );
        assert_eq!(
            store.reading(&work.id).unwrap().page_id.as_deref(),
            Some(old.id.as_str())
        );
        // The same stable ID survives a refreshed hash and preserves the bookmark.
        let mut changed = old.clone();
        changed.source_hash = "b".repeat(64);
        store
            .replace_pages(&unit.id, unit.revision, [changed], &AtomicBool::new(false))
            .unwrap();
        assert_eq!(
            store.reading(&work.id).unwrap().page_id.as_deref(),
            Some(old.id.as_str())
        );
    }
    #[test]
    fn multi_source_registration_is_atomic_and_revision_guarded() {
        let (dir, store, _, _, unit) = setup();
        let sources = [
            (dir.path().join("ok.png"), SourceKind::Image),
            (PathBuf::from("relative.png"), SourceKind::Image),
        ];
        assert_eq!(
            store
                .add_sources(&unit.id, unit.revision, &sources)
                .unwrap_err()
                .code,
            ErrorCode::Input
        );
        assert!(store.sources(&unit.id).unwrap().is_empty());
        assert_eq!(store.unit(&unit.id).unwrap().revision, unit.revision);
        let added = store
            .add_sources(&unit.id, unit.revision, &sources[..1])
            .unwrap();
        assert_eq!(added.len(), 1);
        assert_eq!(
            store
                .add_sources(&unit.id, unit.revision, &sources[..1])
                .unwrap_err()
                .code,
            ErrorCode::Conflict
        );
        assert_eq!(store.sources(&unit.id).unwrap().len(), 1);
    }
    #[test]
    fn page_moves_change_first_page_and_stale_writes_cannot_override_edits() {
        let (dir, store, _, work, unit) = setup();
        let source = store
            .add_source(&unit.id, &dir.path().join("a.png"), SourceKind::Image)
            .unwrap();
        let unit = store.unit(&unit.id).unwrap();
        let a = page(&unit, &source, 1);
        let b = page(&unit, &source, 2);
        let unit = store
            .replace_pages(
                &unit.id,
                unit.revision,
                [a.clone(), b.clone()],
                &AtomicBool::new(false),
            )
            .unwrap();
        let moved = store
            .move_page_before(&unit.id, unit.revision, &b.id, Some(&a.id))
            .unwrap();
        assert_eq!(store.first_page(&work.id).unwrap().unwrap().id, b.id);
        assert_eq!(
            store
                .move_page_before(&unit.id, unit.revision, &a.id, None)
                .unwrap_err()
                .code,
            ErrorCode::Conflict
        );
        assert_eq!(
            store
                .pages(&unit.id, moved.revision, 0, 10)
                .unwrap()
                .items
                .len(),
            2
        );
        let edited = store
            .update_work(&work.id, work.revision, "수정", work.metadata.clone())
            .unwrap();
        assert_eq!(
            store.remove_work(&work.id, work.revision).unwrap_err().code,
            ErrorCode::Conflict
        );
        assert_eq!(store.work(&work.id).unwrap().revision, edited.revision);
    }
    #[test]
    fn legacy_schema_migrates_atomically_and_future_schema_is_preserved() {
        let (dir, store, _, work, unit) = setup();
        let source = store
            .add_source(&unit.id, &dir.path().join("old.png"), SourceKind::Image)
            .unwrap();
        let unit = store.unit(&unit.id).unwrap();
        let old = page(&unit, &source, 1);
        store
            .replace_pages(
                &unit.id,
                unit.revision,
                [old.clone()],
                &AtomicBool::new(false),
            )
            .unwrap();
        store.save_reading(&work.id, Some(&old.id), false).unwrap();
        let db = Connection::open(&store.path).unwrap();
        db.execute_batch("ALTER TABLE pages DROP COLUMN source_hash; ALTER TABLE sources DROP COLUMN position; PRAGMA user_version=1;").unwrap();
        drop(db);
        let migrated = CollectionStore::open(&dir.path().join("library")).unwrap();
        assert_eq!(migrated.page(&old.id).unwrap().source_hash, "");
        assert_eq!(
            migrated.reading(&work.id).unwrap().page_id.as_deref(),
            Some(old.id.as_str())
        );
        assert_eq!(migrated.sources(&unit.id).unwrap().len(), 1);
        let db = Connection::open(&store.path).unwrap();
        db.pragma_update(None, "user_version", 9).unwrap();
        drop(db);
        assert!(
            matches!(CollectionStore::open(&dir.path().join("library")),Err(e) if e.code==ErrorCode::Storage)
        );
        let db = Connection::open(&store.path).unwrap();
        assert_eq!(
            db.pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
                .unwrap(),
            9
        );
        assert_eq!(
            db.query_row("SELECT count(*) FROM pages", [], |row| row.get::<_, u64>(0))
                .unwrap(),
            1
        );
    }
    #[test]
    fn member_paths_and_cross_work_bookmarks_are_rejected() {
        for entry in [
            "../x.png", "/x.png", "C:/x.png", "a\\b.png", "a//b.png", "./x.png",
        ] {
            assert_eq!(
                PageLocation {
                    kind: PageKind::Image,
                    entry: Some(entry.into()),
                    number: 1
                }
                .validate()
                .unwrap_err()
                .code,
                ErrorCode::Input
            );
        }
        let (dir, store, collection, work, unit) = setup();
        let other = store
            .create_work(&collection.id, "別作品", WorkMetadata::default())
            .unwrap();
        let source = store
            .add_source(&unit.id, &dir.path().join("x.png"), SourceKind::Image)
            .unwrap();
        let unit = store.unit(&unit.id).unwrap();
        let p = page(&unit, &source, 1);
        store
            .replace_pages(
                &unit.id,
                unit.revision,
                [p.clone()],
                &AtomicBool::new(false),
            )
            .unwrap();
        assert_eq!(
            store
                .save_reading(&other.id, Some(&p.id), false)
                .unwrap_err()
                .code,
            ErrorCode::Input
        );
        assert_eq!(store.work(&work.id).unwrap().page_count, 1);
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryError {
    pub code: ErrorCode,
    pub message: String,
}
impl std::fmt::Display for LibraryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for LibraryError {}
pub type Result<T> = std::result::Result<T, LibraryError>;
pub fn error(code: ErrorCode) -> LibraryError {
    let message = match code {
        ErrorCode::Input => "The collection request is invalid.",
        ErrorCode::NotFound => "The library item no longer exists.",
        ErrorCode::Conflict => "The library item changed. Reload before saving.",
        ErrorCode::Storage => "The local collections database is unavailable.",
        ErrorCode::SourceChanged => "The referenced source changed or is unavailable.",
        ErrorCode::Cancelled => "The collection operation was cancelled.",
        ErrorCode::Unsupported => "This gallery source format is not supported.",
        ErrorCode::Limit => "The collection worker resource limit was reached.",
        ErrorCode::UnsafePath => "The referenced source path is not allowed.",
        ErrorCode::Busy => "A collection operation is already running.",
        ErrorCode::Encrypted => "The referenced document is password protected.",
    };
    LibraryError {
        code,
        message: message.into(),
    }
}
fn storage(_: impl std::fmt::Display) -> LibraryError {
    error(ErrorCode::Storage)
}
pub fn id() -> Result<String> {
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes).map_err(storage)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}
fn title(value: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 512 || value.chars().any(char::is_control) {
        Err(error(ErrorCode::Input))
    } else {
        Ok(value.into())
    }
}
fn identifier(value: &str) -> Result<()> {
    if value.len() != 32 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        Err(error(ErrorCode::Input))
    } else {
        Ok(())
    }
}
fn revision(value: u64) -> Result<i64> {
    i64::try_from(value).map_err(|_| error(ErrorCode::Input))
}
fn window(offset: u64, limit: u32) -> Result<(i64, i64)> {
    if !(1..=100).contains(&limit) {
        return Err(error(ErrorCode::Input));
    }
    Ok((
        i64::try_from(offset).map_err(|_| error(ErrorCode::Input))?,
        i64::from(limit),
    ))
}
fn encode(value: &impl Serialize) -> Result<String> {
    let text = serde_json::to_string(value).map_err(storage)?;
    if text.len() > 32768 {
        Err(error(ErrorCode::Limit))
    } else {
        Ok(text)
    }
}
fn decode<T: for<'de> Deserialize<'de>>(text: String) -> rusqlite::Result<T> {
    if text.len() > 32768 {
        return Err(rusqlite::Error::InvalidQuery);
    }
    serde_json::from_str(&text).map_err(|_| rusqlite::Error::InvalidQuery)
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum WorkKind {
    Document,
    Image,
    Comic,
    Webtoon,
    Novel,
    Hitomi,
    Other,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ReadingDirection {
    LeftToRight,
    RightToLeft,
    Vertical,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum UnitKind {
    Volume,
    Chapter,
    Part,
    Document,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SourceKind {
    Image,
    Folder,
    Pdf,
    Archive,
    Text,
    Epub,
    Document,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PageKind {
    Image,
    Pdf,
    Text,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Collection {
    pub id: String,
    pub title: String,
    pub description: String,
    pub revision: u64,
    pub created_ms: i64,
    pub updated_ms: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WorkMetadata {
    pub kind: WorkKind,
    pub direction: ReadingDirection,
    pub authors: Vec<String>,
    pub tags: Vec<String>,
    pub description: String,
    /// User-provided catalogue identifier only; it does not initiate a download.
    pub external_id: Option<String>,
}
impl Default for WorkMetadata {
    fn default() -> Self {
        Self {
            kind: WorkKind::Document,
            direction: ReadingDirection::LeftToRight,
            authors: vec![],
            tags: vec![],
            description: String::new(),
            external_id: None,
        }
    }
}
impl WorkMetadata {
    fn validate(&self) -> Result<()> {
        if self.authors.len() > 64
            || self.tags.len() > 256
            || self.description.len() > 16384
            || self.description.contains('\0')
            || self
                .external_id
                .as_ref()
                .is_some_and(|v| v.len() > 256 || v.chars().any(char::is_control))
            || self
                .authors
                .iter()
                .chain(&self.tags)
                .any(|s| s.trim().is_empty() || s.len() > 256 || s.chars().any(char::is_control))
        {
            return Err(error(ErrorCode::Input));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Work {
    pub id: String,
    pub collection_id: String,
    pub title: String,
    pub metadata: WorkMetadata,
    pub position: i64,
    pub revision: u64,
    pub created_ms: i64,
    pub updated_ms: i64,
    pub unit_count: u64,
    pub page_count: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Unit {
    pub id: String,
    pub work_id: String,
    pub title: String,
    pub kind: UnitKind,
    pub position: i64,
    pub revision: u64,
    pub state: String,
    pub page_count: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    pub id: String,
    pub unit_id: String,
    pub path: String,
    pub kind: SourceKind,
    pub revision: u64,
    pub position: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageLocation {
    pub kind: PageKind,
    /// Relative member of a registered folder/archive, never an arbitrary path.
    pub entry: Option<String>,
    /// One-based frame, PDF page or text-section number in the original source.
    pub number: u32,
}
impl PageLocation {
    pub fn validate(&self) -> Result<()> {
        if self.number == 0
            || self.entry.as_ref().is_some_and(|s| {
                s.is_empty()
                    || s.len() > 8192
                    || s.contains(['\0', '\\', ':'])
                    || s.starts_with('/')
                    || s.split('/')
                        .any(|part| part.is_empty() || part == ".." || part == ".")
            })
        {
            return Err(error(ErrorCode::Input));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    pub id: String,
    pub unit_id: String,
    pub source_id: String,
    pub position: u64,
    pub location: PageLocation,
    pub title: String,
    /// SHA-256 of the referenced image/file or whole archive/PDF at import.
    /// The renderer checks it again before returning a cached or fresh page.
    pub source_hash: String,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PageAddress {
    pub page: Page,
    pub unit: Unit,
    pub offset: u64,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Listing<T> {
    pub items: Vec<T>,
    pub total: u64,
    pub next_offset: Option<u64>,
}
fn listing<T>(items: Vec<T>, total: u64, offset: u64) -> Listing<T> {
    let end = offset.saturating_add(items.len() as u64);
    Listing {
        items,
        total,
        next_offset: (end < total).then_some(end),
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadingPosition {
    pub work_id: String,
    pub page_id: Option<String>,
    pub completed: bool,
    pub updated_ms: i64,
}

#[derive(Clone)]
pub struct CollectionStore {
    path: PathBuf,
}
const WORK_SELECT:&str="SELECT w.id,w.collection_id,w.title,w.metadata,w.position,w.revision,w.created_ms,w.updated_ms,(SELECT count(*) FROM units u WHERE u.work_id=w.id),(SELECT count(*) FROM pages p JOIN units u ON p.unit_id=u.id WHERE u.work_id=w.id) FROM works w";
fn read_collection(r: &rusqlite::Row<'_>) -> rusqlite::Result<Collection> {
    Ok(Collection {
        id: r.get(0)?,
        title: r.get(1)?,
        description: r.get(2)?,
        revision: r.get(3)?,
        created_ms: r.get(4)?,
        updated_ms: r.get(5)?,
    })
}
fn read_work(r: &rusqlite::Row<'_>) -> rusqlite::Result<Work> {
    Ok(Work {
        id: r.get(0)?,
        collection_id: r.get(1)?,
        title: r.get(2)?,
        metadata: decode(r.get(3)?)?,
        position: r.get(4)?,
        revision: r.get(5)?,
        created_ms: r.get(6)?,
        updated_ms: r.get(7)?,
        unit_count: r.get(8)?,
        page_count: r.get(9)?,
    })
}
fn read_unit(r: &rusqlite::Row<'_>) -> rusqlite::Result<Unit> {
    Ok(Unit {
        id: r.get(0)?,
        work_id: r.get(1)?,
        title: r.get(2)?,
        kind: decode(r.get(3)?)?,
        position: r.get(4)?,
        revision: r.get(5)?,
        state: r.get(6)?,
        page_count: r.get(7)?,
    })
}
fn read_source(r: &rusqlite::Row<'_>) -> rusqlite::Result<Source> {
    Ok(Source {
        id: r.get(0)?,
        unit_id: r.get(1)?,
        path: r.get(2)?,
        kind: decode(r.get(3)?)?,
        revision: r.get(4)?,
        position: r.get(5)?,
    })
}
fn read_page(r: &rusqlite::Row<'_>) -> rusqlite::Result<Page> {
    Ok(Page {
        id: r.get(0)?,
        unit_id: r.get(1)?,
        source_id: r.get(2)?,
        position: r.get(3)?,
        location: decode(r.get(4)?)?,
        title: r.get(5)?,
        source_hash: r.get(6)?,
    })
}
fn changed(db: &Connection, table: &str, id: &str, expected: u64) -> Result<()> {
    revision(expected)?;
    let found: Option<u64> = db
        .query_row(
            &format!("SELECT revision FROM {table} WHERE id=?1"),
            [id],
            |r| r.get(0),
        )
        .optional()
        .map_err(storage)?;
    match found {
        None => Err(error(ErrorCode::NotFound)),
        Some(value) if value != expected => Err(error(ErrorCode::Conflict)),
        _ => Ok(()),
    }
}
impl CollectionStore {
    pub fn open(directory: &Path) -> Result<Self> {
        fs::create_dir_all(directory).map_err(storage)?;
        let store = Self {
            path: directory.join("collections.sqlite3"),
        };
        let mut db = store.connect()?;
        let version: u32 = db
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .map_err(storage)?;
        if version > 2 {
            return Err(error(ErrorCode::Storage));
        }
        db.pragma_update(None, "journal_mode", "WAL")
            .map_err(storage)?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        tx.execute_batch("
            CREATE TABLE IF NOT EXISTS collections(id TEXT PRIMARY KEY,title TEXT NOT NULL,description TEXT NOT NULL,revision INTEGER NOT NULL,created_ms INTEGER NOT NULL,updated_ms INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS works(id TEXT PRIMARY KEY,collection_id TEXT NOT NULL REFERENCES collections(id) ON DELETE CASCADE,title TEXT NOT NULL,metadata TEXT NOT NULL,position INTEGER NOT NULL,revision INTEGER NOT NULL,created_ms INTEGER NOT NULL,updated_ms INTEGER NOT NULL);
            CREATE INDEX IF NOT EXISTS works_order ON works(collection_id,position,id);
            CREATE TABLE IF NOT EXISTS units(id TEXT PRIMARY KEY,work_id TEXT NOT NULL REFERENCES works(id) ON DELETE CASCADE,title TEXT NOT NULL,kind TEXT NOT NULL,position INTEGER NOT NULL,revision INTEGER NOT NULL,state TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS units_order ON units(work_id,position,id);
            CREATE TABLE IF NOT EXISTS sources(id TEXT PRIMARY KEY,unit_id TEXT NOT NULL REFERENCES units(id) ON DELETE CASCADE,path TEXT NOT NULL,kind TEXT NOT NULL,revision INTEGER NOT NULL,position INTEGER NOT NULL);
            CREATE INDEX IF NOT EXISTS sources_unit ON sources(unit_id,id);
            CREATE TABLE IF NOT EXISTS pages(id TEXT PRIMARY KEY,unit_id TEXT NOT NULL REFERENCES units(id) ON DELETE CASCADE,source_id TEXT NOT NULL REFERENCES sources(id) ON DELETE CASCADE,position INTEGER NOT NULL,location TEXT NOT NULL,title TEXT NOT NULL,source_hash TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS pages_order ON pages(unit_id,position,id);
            CREATE TABLE IF NOT EXISTS reading(work_id TEXT PRIMARY KEY REFERENCES works(id) ON DELETE CASCADE,page_id TEXT REFERENCES pages(id) ON DELETE SET NULL,completed INTEGER NOT NULL,updated_ms INTEGER NOT NULL);
            ").map_err(storage)?;
        if version == 1 {
            // Old drafts have no authenticated source hash. Keep their metadata,
            // but an empty hash requires a fresh import before rendering.
            tx.execute_batch("ALTER TABLE pages ADD COLUMN source_hash TEXT NOT NULL DEFAULT ''; ALTER TABLE sources ADD COLUMN position INTEGER NOT NULL DEFAULT 0; UPDATE sources SET position=rowid;")
                .map_err(storage)?;
        }
        tx.pragma_update(None, "user_version", 2).map_err(storage)?;
        tx.commit().map_err(storage)?;
        Ok(store)
    }
    fn connect(&self) -> Result<Connection> {
        let db = Connection::open(&self.path).map_err(storage)?;
        db.busy_timeout(Duration::from_secs(3)).map_err(storage)?;
        db.execute_batch("PRAGMA foreign_keys=ON;")
            .map_err(storage)?;
        Ok(db)
    }
    pub fn create_collection(&self, name: &str, description: &str) -> Result<Collection> {
        let name = title(name)?;
        if description.len() > 16384 || description.contains('\0') {
            return Err(error(ErrorCode::Input));
        }
        let id = id()?;
        let now = now();
        self.connect()?
            .execute(
                "INSERT INTO collections VALUES(?1,?2,?3,1,?4,?4)",
                params![id, name, description, now],
            )
            .map_err(storage)?;
        self.collection(&id)
    }
    pub fn collection(&self, id: &str) -> Result<Collection> {
        identifier(id)?;
        self.connect()?.query_row("SELECT id,title,description,revision,created_ms,updated_ms FROM collections WHERE id=?1",[id],read_collection).optional().map_err(storage)?.ok_or_else(||error(ErrorCode::NotFound))
    }
    pub fn collections(&self, offset: u64, limit: u32) -> Result<Listing<Collection>> {
        let (start, count) = window(offset, limit)?;
        let db = self.connect()?;
        let total = db
            .query_row("SELECT count(*) FROM collections", [], |r| r.get(0))
            .map_err(storage)?;
        let items=db.prepare("SELECT id,title,description,revision,created_ms,updated_ms FROM collections ORDER BY created_ms,id LIMIT ?1 OFFSET ?2").map_err(storage)?.query_map(params![count,start],read_collection).map_err(storage)?.collect::<rusqlite::Result<Vec<_>>>().map_err(storage)?;
        Ok(listing(items, total, offset))
    }
    pub fn update_collection(
        &self,
        id: &str,
        expected: u64,
        name: &str,
        description: &str,
    ) -> Result<Collection> {
        identifier(id)?;
        let name = title(name)?;
        if description.len() > 16384 || description.contains('\0') {
            return Err(error(ErrorCode::Input));
        }
        let mut db = self.connect()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        changed(&tx, "collections", id, expected)?;
        tx.execute("UPDATE collections SET title=?1,description=?2,revision=revision+1,updated_ms=?3 WHERE id=?4",params![name,description,now(),id]).map_err(storage)?;
        tx.commit().map_err(storage)?;
        self.collection(id)
    }
    pub fn create_work(
        &self,
        collection_id: &str,
        name: &str,
        metadata: WorkMetadata,
    ) -> Result<Work> {
        self.collection(collection_id)?;
        let name = title(name)?;
        metadata.validate()?;
        let id = id()?;
        let now = now();
        self.connect()?.execute("INSERT INTO works SELECT ?1,?2,?3,?4,coalesce(max(position)+1,0),1,?5,?5 FROM works WHERE collection_id=?2",params![id,collection_id,name,encode(&metadata)?,now]).map_err(storage)?;
        self.work(&id)
    }
    pub fn work(&self, id: &str) -> Result<Work> {
        identifier(id)?;
        self.connect()?
            .query_row(&format!("{WORK_SELECT} WHERE w.id=?1"), [id], read_work)
            .optional()
            .map_err(storage)?
            .ok_or_else(|| error(ErrorCode::NotFound))
    }
    pub fn works(&self, collection_id: &str, offset: u64, limit: u32) -> Result<Listing<Work>> {
        self.collection(collection_id)?;
        let (start, count) = window(offset, limit)?;
        let db = self.connect()?;
        let total = db
            .query_row(
                "SELECT count(*) FROM works WHERE collection_id=?1",
                [collection_id],
                |r| r.get(0),
            )
            .map_err(storage)?;
        let items=db.prepare(&format!("{WORK_SELECT} WHERE w.collection_id=?1 ORDER BY w.position,w.id LIMIT ?2 OFFSET ?3")).map_err(storage)?.query_map(params![collection_id,count,start],read_work).map_err(storage)?.collect::<rusqlite::Result<Vec<_>>>().map_err(storage)?;
        Ok(listing(items, total, offset))
    }
    pub fn update_work(
        &self,
        id: &str,
        expected: u64,
        name: &str,
        metadata: WorkMetadata,
    ) -> Result<Work> {
        identifier(id)?;
        let name = title(name)?;
        metadata.validate()?;
        let mut db = self.connect()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        changed(&tx, "works", id, expected)?;
        tx.execute(
            "UPDATE works SET title=?1,metadata=?2,revision=revision+1,updated_ms=?3 WHERE id=?4",
            params![name, encode(&metadata)?, now(), id],
        )
        .map_err(storage)?;
        tx.commit().map_err(storage)?;
        self.work(id)
    }
    pub fn create_unit(&self, work_id: &str, name: &str, kind: UnitKind) -> Result<Unit> {
        self.work(work_id)?;
        let name = title(name)?;
        let id = id()?;
        self.connect()?.execute("INSERT INTO units SELECT ?1,?2,?3,?4,coalesce(max(position)+1,0),1,'empty' FROM units WHERE work_id=?2",params![id,work_id,name,encode(&kind)?]).map_err(storage)?;
        self.unit(&id)
    }
    pub fn unit(&self, id: &str) -> Result<Unit> {
        identifier(id)?;
        self.connect()?.query_row("SELECT id,work_id,title,kind,position,revision,state,(SELECT count(*) FROM pages p WHERE p.unit_id=units.id) FROM units WHERE id=?1",[id],read_unit).optional().map_err(storage)?.ok_or_else(||error(ErrorCode::NotFound))
    }
    pub fn units(&self, work_id: &str, offset: u64, limit: u32) -> Result<Listing<Unit>> {
        self.work(work_id)?;
        let (start, count) = window(offset, limit)?;
        let db = self.connect()?;
        let total = db
            .query_row(
                "SELECT count(*) FROM units WHERE work_id=?1",
                [work_id],
                |r| r.get(0),
            )
            .map_err(storage)?;
        let items=db.prepare("SELECT id,work_id,title,kind,position,revision,state,(SELECT count(*) FROM pages p WHERE p.unit_id=units.id) FROM units WHERE work_id=?1 ORDER BY position,id LIMIT ?2 OFFSET ?3").map_err(storage)?.query_map(params![work_id,count,start],read_unit).map_err(storage)?.collect::<rusqlite::Result<Vec<_>>>().map_err(storage)?;
        Ok(listing(items, total, offset))
    }
    #[cfg(test)]
    pub fn add_source(&self, unit_id: &str, path: &Path, kind: SourceKind) -> Result<Source> {
        let expected = self.unit(unit_id)?.revision;
        self.add_sources(unit_id, expected, &[(path.to_owned(), kind)])?
            .pop()
            .ok_or_else(|| error(ErrorCode::Storage))
    }
    /// References are registered as one revision. No original is opened, moved
    /// or deleted here; the import service authorizes and fingerprints them.
    pub fn add_sources(
        &self,
        unit_id: &str,
        expected: u64,
        sources: &[(PathBuf, SourceKind)],
    ) -> Result<Vec<Source>> {
        identifier(unit_id)?;
        if sources.is_empty() || sources.len() > 10_000 {
            return Err(error(ErrorCode::Input));
        }
        let mut db = self.connect()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        changed(&tx, "units", unit_id, expected)?;
        let count: u64 = tx
            .query_row(
                "SELECT count(*) FROM sources WHERE unit_id=?1",
                [unit_id],
                |row| row.get(0),
            )
            .map_err(storage)?;
        if count.saturating_add(sources.len() as u64) > 10_000 {
            return Err(error(ErrorCode::Limit));
        }
        let mut added = Vec::new();
        let next: i64 = tx
            .query_row(
                "SELECT coalesce(max(position)+1,0) FROM sources WHERE unit_id=?1",
                [unit_id],
                |row| row.get(0),
            )
            .map_err(storage)?;
        for (path, kind) in sources {
            let path = path
                .to_str()
                .filter(|p| !p.contains('\0') && p.len() <= 32768 && Path::new(p).is_absolute())
                .ok_or_else(|| error(ErrorCode::Input))?;
            let id = id()?;
            let position = next
                .checked_add(added.len() as i64)
                .ok_or_else(|| error(ErrorCode::Limit))?;
            tx.execute(
                "INSERT INTO sources(id,unit_id,path,kind,revision,position) VALUES(?1,?2,?3,?4,1,?5)",
                params![id, unit_id, path, encode(kind)?,position],
            )
            .map_err(storage)?;
            added.push(Source {
                id,
                unit_id: unit_id.into(),
                path: path.into(),
                kind: kind.clone(),
                revision: 1,
                position,
            });
        }
        tx.execute(
            "UPDATE units SET revision=revision+1,state='pending' WHERE id=?1",
            [unit_id],
        )
        .map_err(storage)?;
        tx.commit().map_err(storage)?;
        Ok(added)
    }
    pub fn source(&self, id: &str) -> Result<Source> {
        identifier(id)?;
        self.connect()?
            .query_row(
                "SELECT id,unit_id,path,kind,revision,position FROM sources WHERE id=?1",
                [id],
                read_source,
            )
            .optional()
            .map_err(storage)?
            .ok_or_else(|| error(ErrorCode::NotFound))
    }
    pub fn sources(&self, unit_id: &str) -> Result<Vec<Source>> {
        self.unit(unit_id)?;
        self.connect()?
            .prepare(
                "SELECT id,unit_id,path,kind,revision,position FROM sources WHERE unit_id=?1 ORDER BY position,id",
            )
            .map_err(storage)?
            .query_map([unit_id], read_source)
            .map_err(storage)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(storage)
    }

    /// Atomically replace a unit's manifest. Existing page IDs retain reading
    /// positions; a cancelled/stale worker cannot publish half of a new book.
    pub fn replace_pages(
        &self,
        unit_id: &str,
        expected: u64,
        pages: impl IntoIterator<Item = Page>,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<Unit> {
        use std::sync::atomic::Ordering;
        identifier(unit_id)?;
        let mut db = self.connect()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        changed(&tx, "units", unit_id, expected)?;
        tx.execute_batch("CREATE TEMP TABLE incoming_pages(id TEXT PRIMARY KEY);")
            .map_err(storage)?;
        for (position, page) in pages.into_iter().enumerate() {
            if cancel.load(Ordering::Acquire) {
                return Err(error(ErrorCode::Cancelled));
            }
            if page.unit_id != unit_id
                || page.id.len() != 64
                || !page.id.bytes().all(|b| b.is_ascii_hexdigit())
                || page.title.len() > 1024
                || page.source_hash.len() != 64
                || !page
                    .source_hash
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit())
                || position >= 250_000
            {
                return Err(error(ErrorCode::Input));
            }
            page.location.validate()?;
            let belongs: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sources WHERE id=?1 AND unit_id=?2)",
                    params![page.source_id, unit_id],
                    |r| r.get(0),
                )
                .map_err(storage)?;
            if !belongs {
                return Err(error(ErrorCode::Input));
            }
            tx.execute("INSERT INTO incoming_pages VALUES(?1)", [&page.id])
                .map_err(|_| error(ErrorCode::Input))?;
            let other: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM pages WHERE id=?1 AND unit_id<>?2)",
                    params![page.id, unit_id],
                    |r| r.get(0),
                )
                .map_err(storage)?;
            if other {
                return Err(error(ErrorCode::Input));
            }
            tx.execute("INSERT INTO pages(id,unit_id,source_id,position,location,title,source_hash) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(id) DO UPDATE SET source_id=excluded.source_id,position=excluded.position,location=excluded.location,title=excluded.title,source_hash=excluded.source_hash",params![page.id,unit_id,page.source_id,position as i64,encode(&page.location)?,page.title,page.source_hash]).map_err(storage)?;
        }
        if cancel.load(Ordering::Acquire) {
            return Err(error(ErrorCode::Cancelled));
        }
        tx.execute(
            "DELETE FROM pages WHERE unit_id=?1 AND id NOT IN (SELECT id FROM incoming_pages)",
            [unit_id],
        )
        .map_err(storage)?;
        tx.execute(
            "UPDATE units SET revision=revision+1,state='ready' WHERE id=?1",
            [unit_id],
        )
        .map_err(storage)?;
        tx.commit().map_err(storage)?;
        self.unit(unit_id)
    }
    pub fn pages(
        &self,
        unit_id: &str,
        expected: u64,
        offset: u64,
        limit: u32,
    ) -> Result<Listing<Page>> {
        identifier(unit_id)?;
        let (start, count) = window(offset, limit)?;
        let mut db = self.connect()?;
        let tx = db.transaction().map_err(storage)?;
        changed(&tx, "units", unit_id, expected)?;
        let total = tx
            .query_row(
                "SELECT count(*) FROM pages WHERE unit_id=?1",
                [unit_id],
                |r| r.get(0),
            )
            .map_err(storage)?;
        let items=tx.prepare("SELECT id,unit_id,source_id,position,location,title,source_hash FROM pages WHERE unit_id=?1 ORDER BY position,id LIMIT ?2 OFFSET ?3").map_err(storage)?.query_map(params![unit_id,count,start],read_page).map_err(storage)?.collect::<rusqlite::Result<Vec<_>>>().map_err(storage)?;
        Ok(listing(items, total, offset))
    }
    pub fn page(&self, id: &str) -> Result<Page> {
        if id.len() != 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(error(ErrorCode::Input));
        }
        self.connect()?
            .query_row(
                "SELECT id,unit_id,source_id,position,location,title,source_hash FROM pages WHERE id=?1",
                [id],
                read_page,
            )
            .optional()
            .map_err(storage)?
            .ok_or_else(|| error(ErrorCode::NotFound))
    }
    pub fn first_page(&self, work_id: &str) -> Result<Option<Page>> {
        self.work(work_id)?;
        self.connect()?.query_row("SELECT p.id,p.unit_id,p.source_id,p.position,p.location,p.title,p.source_hash FROM pages p JOIN units u ON p.unit_id=u.id WHERE u.work_id=?1 ORDER BY u.position,u.id,p.position,p.id LIMIT 1",[work_id],read_page).optional().map_err(storage)
    }
    pub fn page_address(&self, work_id: &str, page_id: &str) -> Result<PageAddress> {
        identifier(work_id)?;
        if page_id.len() != 64 || !page_id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(error(ErrorCode::Input));
        }
        let mut db = self.connect()?;
        let tx = db.transaction().map_err(storage)?;
        let page=tx.query_row("SELECT id,unit_id,source_id,position,location,title,source_hash FROM pages WHERE id=?1",[page_id],read_page).optional().map_err(storage)?.ok_or_else(||error(ErrorCode::NotFound))?;
        let unit=tx.query_row("SELECT id,work_id,title,kind,position,revision,state,(SELECT count(*) FROM pages p WHERE p.unit_id=units.id) FROM units WHERE id=?1",[&page.unit_id],read_unit).map_err(storage)?;
        if unit.work_id != work_id {
            return Err(error(ErrorCode::Input));
        }
        let offset=tx.query_row("SELECT count(*) FROM pages WHERE unit_id=?1 AND (position<?2 OR (position=?2 AND id<?3))",params![unit.id,page.position,page.id],|row|row.get(0)).map_err(storage)?;
        Ok(PageAddress { page, unit, offset })
    }
    pub fn failed_import(&self, unit_id: &str, expected: u64, cancelled: bool) -> Result<()> {
        identifier(unit_id)?;
        let mut db = self.connect()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        changed(&tx, "units", unit_id, expected)?;
        tx.execute(
            "UPDATE units SET revision=revision+1,state=?1 WHERE id=?2",
            params![if cancelled { "cancelled" } else { "failed" }, unit_id],
        )
        .map_err(storage)?;
        tx.commit().map_err(storage)
    }
    pub fn save_reading(
        &self,
        work_id: &str,
        page_id: Option<&str>,
        completed: bool,
    ) -> Result<ReadingPosition> {
        self.work(work_id)?;
        let mut db = self.connect()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        if let Some(page) = page_id {
            let belongs:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM pages p JOIN units u ON p.unit_id=u.id WHERE p.id=?1 AND u.work_id=?2)",params![page,work_id],|r|r.get(0)).map_err(storage)?;
            if !belongs {
                return Err(error(ErrorCode::Input));
            }
        }
        tx.execute("INSERT INTO reading VALUES(?1,?2,?3,?4) ON CONFLICT(work_id) DO UPDATE SET page_id=excluded.page_id,completed=excluded.completed,updated_ms=excluded.updated_ms",params![work_id,page_id,completed,now()]).map_err(storage)?;
        tx.commit().map_err(storage)?;
        self.reading(work_id)
    }
    pub fn reading(&self, work_id: &str) -> Result<ReadingPosition> {
        self.work(work_id)?;
        Ok(self
            .connect()?
            .query_row(
                "SELECT work_id,page_id,completed,updated_ms FROM reading WHERE work_id=?1",
                [work_id],
                |r| {
                    Ok(ReadingPosition {
                        work_id: r.get(0)?,
                        page_id: r.get(1)?,
                        completed: r.get(2)?,
                        updated_ms: r.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(storage)?
            .unwrap_or(ReadingPosition {
                work_id: work_id.into(),
                page_id: None,
                completed: false,
                updated_ms: 0,
            }))
    }
    pub fn update_unit(&self, id: &str, expected: u64, name: &str, kind: UnitKind) -> Result<Unit> {
        identifier(id)?;
        let name = title(name)?;
        let mut db = self.connect()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        changed(&tx, "units", id, expected)?;
        tx.execute(
            "UPDATE units SET title=?1,kind=?2,revision=revision+1 WHERE id=?3",
            params![name, encode(&kind)?, id],
        )
        .map_err(storage)?;
        tx.commit().map_err(storage)?;
        self.unit(id)
    }
    pub fn move_work(&self, id: &str, expected: u64, collection_id: &str) -> Result<Work> {
        identifier(id)?;
        identifier(collection_id)?;
        let mut db = self.connect()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        changed(&tx, "works", id, expected)?;
        let exists: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM collections WHERE id=?1)",
                [collection_id],
                |r| r.get(0),
            )
            .map_err(storage)?;
        if !exists {
            return Err(error(ErrorCode::NotFound));
        }
        tx.execute("UPDATE works SET collection_id=?1,position=(SELECT coalesce(max(position)+1,0) FROM works WHERE collection_id=?1),revision=revision+1,updated_ms=?2 WHERE id=?3",params![collection_id,now(),id]).map_err(storage)?;
        tx.commit().map_err(storage)?;
        self.work(id)
    }
    pub fn move_unit_before(&self, id: &str, expected: u64, before: Option<&str>) -> Result<Unit> {
        identifier(id)?;
        let mut db = self.connect()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        changed(&tx, "units", id, expected)?;
        let work: String = tx
            .query_row("SELECT work_id FROM units WHERE id=?1", [id], |r| r.get(0))
            .map_err(storage)?;
        if before != Some(id) {
            let position = if let Some(before) = before {
                identifier(before)?;
                tx.query_row(
                    "SELECT position FROM units WHERE id=?1 AND work_id=?2",
                    params![before, work],
                    |r| r.get::<_, i64>(0),
                )
                .optional()
                .map_err(storage)?
                .ok_or_else(|| error(ErrorCode::Input))?
            } else {
                tx.query_row(
                    "SELECT coalesce(max(position)+1,0) FROM units WHERE work_id=?1",
                    [&work],
                    |r| r.get(0),
                )
                .map_err(storage)?
            };
            tx.execute(
                "UPDATE units SET position=position+1 WHERE work_id=?1 AND position>=?2",
                params![work, position],
            )
            .map_err(storage)?;
            tx.execute(
                "UPDATE units SET position=?1,revision=revision+1 WHERE id=?2",
                params![position, id],
            )
            .map_err(storage)?;
        }
        tx.commit().map_err(storage)?;
        self.unit(id)
    }
    pub fn move_page_before(
        &self,
        unit_id: &str,
        expected: u64,
        page_id: &str,
        before: Option<&str>,
    ) -> Result<Unit> {
        identifier(unit_id)?;
        let mut db = self.connect()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        changed(&tx, "units", unit_id, expected)?;
        let belongs: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM pages WHERE id=?1 AND unit_id=?2)",
                params![page_id, unit_id],
                |r| r.get(0),
            )
            .map_err(storage)?;
        if !belongs {
            return Err(error(ErrorCode::Input));
        }
        if before != Some(page_id) {
            let position = if let Some(before) = before {
                tx.query_row(
                    "SELECT position FROM pages WHERE id=?1 AND unit_id=?2",
                    params![before, unit_id],
                    |r| r.get::<_, i64>(0),
                )
                .optional()
                .map_err(storage)?
                .ok_or_else(|| error(ErrorCode::Input))?
            } else {
                tx.query_row(
                    "SELECT coalesce(max(position)+1,0) FROM pages WHERE unit_id=?1",
                    [unit_id],
                    |r| r.get(0),
                )
                .map_err(storage)?
            };
            tx.execute(
                "UPDATE pages SET position=position+1 WHERE unit_id=?1 AND position>=?2",
                params![unit_id, position],
            )
            .map_err(storage)?;
            tx.execute(
                "UPDATE pages SET position=?1 WHERE id=?2",
                params![position, page_id],
            )
            .map_err(storage)?;
            tx.execute(
                "UPDATE units SET revision=revision+1 WHERE id=?1",
                [unit_id],
            )
            .map_err(storage)?;
        }
        tx.commit().map_err(storage)?;
        self.unit(unit_id)
    }
    pub fn relink_source(
        &self,
        id: &str,
        expected: u64,
        path: &Path,
        kind: SourceKind,
    ) -> Result<Source> {
        identifier(id)?;
        let path = path
            .to_str()
            .filter(|p| !p.contains('\0') && p.len() <= 32768 && Path::new(p).is_absolute())
            .ok_or_else(|| error(ErrorCode::Input))?;
        let mut db = self.connect()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        changed(&tx, "sources", id, expected)?;
        tx.execute(
            "UPDATE sources SET path=?1,kind=?2,revision=revision+1 WHERE id=?3",
            params![path, encode(&kind)?, id],
        )
        .map_err(storage)?;
        tx.execute("UPDATE units SET revision=revision+1,state='pending' WHERE id=(SELECT unit_id FROM sources WHERE id=?1)",[id]).map_err(storage)?;
        tx.commit().map_err(storage)?;
        self.source(id)
    }
    pub fn remove_source(&self, id: &str, expected: u64) -> Result<()> {
        identifier(id)?;
        let mut db = self.connect()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        changed(&tx, "sources", id, expected)?;
        tx.execute("UPDATE units SET revision=revision+1,state='pending' WHERE id=(SELECT unit_id FROM sources WHERE id=?1)",[id]).map_err(storage)?;
        tx.execute("DELETE FROM sources WHERE id=?1", [id])
            .map_err(storage)?;
        tx.commit().map_err(storage)
    }
    pub fn remove_collection(&self, id: &str, expected: u64) -> Result<()> {
        self.remove("collections", id, expected)
    }
    pub fn remove_work(&self, id: &str, expected: u64) -> Result<()> {
        self.remove("works", id, expected)
    }
    pub fn remove_unit(&self, id: &str, expected: u64) -> Result<()> {
        self.remove("units", id, expected)
    }
    fn remove(&self, table: &str, id: &str, expected: u64) -> Result<()> {
        identifier(id)?;
        let mut db = self.connect()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        changed(&tx, table, id, expected)?;
        tx.execute(&format!("DELETE FROM {table} WHERE id=?1"), [id])
            .map_err(storage)?;
        tx.commit().map_err(storage)
    }
}
