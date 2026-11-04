use crate::document_service::*;
use crate::rag_service::{RagError, ScopeSnapshot};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const MAX_RECORD: usize = 2 * 1024 * 1024;
#[derive(Clone)]
pub(crate) struct DocumentStore {
    path: PathBuf,
}
pub(crate) fn fresh_id() -> DocumentResult<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(storage)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}
pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |value| value.as_millis().min(i64::MAX as u128) as u64)
}
fn encode(revision: &DocumentRevision) -> DocumentResult<String> {
    let value = serde_json::to_string(revision).map_err(storage)?;
    if value.len() > MAX_RECORD {
        return Err(DocumentError::new(DocumentErrorCode::Limit));
    }
    Ok(value)
}
fn decode(value: String) -> DocumentResult<DocumentRevision> {
    if value.len() > MAX_RECORD {
        return Err(DocumentError::new(DocumentErrorCode::Limit));
    }
    let mut revision: DocumentRevision = serde_json::from_str(&value).map_err(storage)?;
    revision.claims_verified = false;
    for block in &mut revision.blocks {
        block.claims_verified = false;
    }
    Ok(revision)
}
impl DocumentStore {
    pub fn open(directory: &Path) -> DocumentResult<Self> {
        fs::create_dir_all(directory).map_err(storage)?;
        let store = Self {
            path: directory.join("documents.sqlite3"),
        };
        let db = store.connect()?;
        let version: u32 = db
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(storage)?;
        if version > 1 {
            return Err(DocumentError::new(DocumentErrorCode::Storage));
        }
        db.execute_batch("PRAGMA journal_mode=WAL;
            CREATE TABLE IF NOT EXISTS documents(id TEXT PRIMARY KEY,title TEXT NOT NULL,purpose TEXT NOT NULL,rag_conversation_id TEXT NOT NULL UNIQUE,created_ms INTEGER NOT NULL,updated_ms INTEGER NOT NULL,current_revision_id TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS document_revisions(id TEXT PRIMARY KEY,document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,sequence INTEGER NOT NULL,payload TEXT NOT NULL,UNIQUE(document_id,sequence));
            CREATE INDEX IF NOT EXISTS document_revisions_order ON document_revisions(document_id,sequence);
            CREATE TABLE IF NOT EXISTS document_jobs(request_id TEXT PRIMARY KEY,document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,created_ms INTEGER NOT NULL,phase TEXT NOT NULL,error TEXT);
            CREATE INDEX IF NOT EXISTS document_jobs_order ON document_jobs(document_id,created_ms);
            UPDATE document_jobs SET phase='interrupted' WHERE phase IN ('outline','body','rewrite');
            PRAGMA user_version=1;").map_err(storage)?;
        Ok(store)
    }
    fn connect(&self) -> DocumentResult<Connection> {
        let db = Connection::open(&self.path).map_err(storage)?;
        db.busy_timeout(Duration::from_secs(3)).map_err(storage)?;
        db.execute_batch("PRAGMA foreign_keys=ON;")
            .map_err(storage)?;
        Ok(db)
    }
    pub fn create(
        &self,
        input: CreateDocument,
        snapshot: ScopeSnapshot,
        conversation: &str,
    ) -> DocumentResult<DocumentPage> {
        let id = fresh_id()?;
        let revision_id = fresh_id()?;
        let now = now_ms();
        let revision = DocumentRevision {
            id: revision_id.clone(),
            document_id: id.clone(),
            sequence: 1,
            parent_revision_id: None,
            created_ms: now,
            operation: DocumentOperation::Create,
            plan: DocumentPlan {
                title: input.title.clone(),
                sections: vec![],
            },
            outline_reviewed: false,
            blocks: vec![],
            markdown: String::new(),
            scope_ids: input.scope_ids,
            external_input_eligible: snapshot.external_questions_allowed,
            scope_snapshot: snapshot,
            citations: vec![],
            provenance: vec![],
            rag_request_id: None,
            provider: None,
            requested_model: None,
            reported_model: None,
            usage: None,
            claims_verified: false,
        };
        let purpose = if let Some(template) = input.template_markdown {
            serde_json::json!({"purpose":input.purpose,"templateMarkdown":template}).to_string()
        } else {
            input.purpose
        };
        let mut db = self.connect()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        tx.execute("INSERT INTO documents(id,title,purpose,rag_conversation_id,created_ms,updated_ms,current_revision_id) VALUES(?1,?2,?3,?4,?5,?5,?6)",params![id,input.title,purpose,conversation,now,revision_id]).map_err(storage)?;
        tx.execute(
            "INSERT INTO document_revisions(id,document_id,sequence,payload) VALUES(?1,?2,1,?3)",
            params![revision.id, id, encode(&revision)?],
        )
        .map_err(storage)?;
        tx.commit().map_err(storage)?;
        self.get(&id, None)
    }
    pub fn get(&self, id: &str, revision_id: Option<&str>) -> DocumentResult<DocumentPage> {
        let db = self.connect()?;
        let document = self.summary_with(&db, id)?;
        let payload: String = db
            .query_row(
                "SELECT payload FROM document_revisions WHERE id=?1 AND document_id=?2",
                params![revision_id.unwrap_or(&document.current_revision_id), id],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)?
            .ok_or_else(|| DocumentError::new(DocumentErrorCode::NotFound))?;
        Ok(DocumentPage {
            document,
            revision: decode(payload)?,
        })
    }
    fn summary_with(&self, db: &Connection, id: &str) -> DocumentResult<DocumentSummary> {
        let mut summary=db.query_row("SELECT d.id,d.title,d.created_ms,d.updated_ms,d.current_revision_id,(SELECT count(*) FROM document_revisions r WHERE r.document_id=d.id) FROM documents d WHERE d.id=?1",[id],read_summary).optional().map_err(storage)?.ok_or_else(||DocumentError::new(DocumentErrorCode::NotFound))?;
        let payload: String = db
            .query_row(
                "SELECT payload FROM document_revisions WHERE id=?1",
                [&summary.current_revision_id],
                |row| row.get(0),
            )
            .map_err(storage)?;
        summary.has_reviewed_outline = decode(payload)?.outline_reviewed;
        Ok(summary)
    }
    pub fn list(&self, query: &str, limit: usize) -> DocumentResult<Vec<DocumentSummary>> {
        let db = self.connect()?;
        let ids: Vec<String> = {
            let mut statement=db.prepare("SELECT id FROM documents WHERE ?1='' OR instr(lower(title),lower(?1))>0 OR instr(lower(purpose),lower(?1))>0 ORDER BY updated_ms DESC,id LIMIT ?2").map_err(storage)?;
            let rows = statement
                .query_map(params![query, limit as u64], |row| row.get(0))
                .map_err(storage)?;
            rows.collect::<Result<_, _>>().map_err(storage)?
        };
        ids.iter().map(|id| self.summary_with(&db, id)).collect()
    }
    pub fn revisions(
        &self,
        id: &str,
        before: Option<u64>,
        limit: usize,
    ) -> DocumentResult<Vec<DocumentRevisionSummary>> {
        if before.is_some_and(|value| value > i64::MAX as u64) {
            return Err(DocumentError::new(DocumentErrorCode::Input));
        }
        let db = self.connect()?;
        self.summary_with(&db, id)?;
        let mut query=db.prepare("SELECT payload FROM document_revisions WHERE document_id=?1 AND (?2 IS NULL OR sequence<?2) ORDER BY sequence DESC LIMIT ?3").map_err(storage)?;
        let rows = query
            .query_map(params![id, before, limit as u64], |row| {
                row.get::<_, String>(0)
            })
            .map_err(storage)?;
        let mut result = Vec::new();
        let mut bytes = 0usize;
        for row in rows {
            let payload = row.map_err(storage)?;
            bytes += payload.len();
            if bytes > 8 * 1024 * 1024 {
                break;
            }
            let value = decode(payload)?;
            result.push(DocumentRevisionSummary {
                id: value.id,
                sequence: value.sequence,
                parent_revision_id: value.parent_revision_id,
                created_ms: value.created_ms,
                operation: value.operation,
            });
        }
        Ok(result)
    }
    pub fn rename(&self, id: &str, title: &str) -> DocumentResult<()> {
        let count = self
            .connect()?
            .execute(
                "UPDATE documents SET title=?1,updated_ms=?2 WHERE id=?3",
                params![title, now_ms(), id],
            )
            .map_err(storage)?;
        if count == 0 {
            Err(DocumentError::new(DocumentErrorCode::NotFound))
        } else {
            Ok(())
        }
    }
    pub fn delete(&self, id: &str) -> DocumentResult<()> {
        self.connect()?
            .execute("DELETE FROM documents WHERE id=?1", [id])
            .map_err(storage)?;
        Ok(())
    }
    pub fn conversation(&self, id: &str) -> DocumentResult<String> {
        self.connect()?
            .query_row(
                "SELECT rag_conversation_id FROM documents WHERE id=?1",
                [id],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)?
            .ok_or_else(|| DocumentError::new(DocumentErrorCode::NotFound))
    }
    pub fn purpose(&self, id: &str) -> DocumentResult<String> {
        self.connect()?
            .query_row("SELECT purpose FROM documents WHERE id=?1", [id], |row| {
                row.get(0)
            })
            .optional()
            .map_err(storage)?
            .ok_or_else(|| DocumentError::new(DocumentErrorCode::NotFound))
    }
    pub fn reserve_job(
        &self,
        id: &str,
        expected: &str,
        request: &str,
        operation: DocumentOperation,
    ) -> DocumentResult<()> {
        let mut db = self.connect()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        let current: String = tx
            .query_row(
                "SELECT current_revision_id FROM documents WHERE id=?1",
                [id],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)?
            .ok_or_else(|| DocumentError::new(DocumentErrorCode::NotFound))?;
        if current != expected {
            return Err(DocumentError::new(DocumentErrorCode::Conflict));
        }
        let duplicate: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM document_jobs WHERE request_id=?1)",
                [request],
                |row| row.get(0),
            )
            .map_err(storage)?;
        if duplicate {
            return Err(DocumentError::new(DocumentErrorCode::Conflict));
        }
        let phase = match operation {
            DocumentOperation::Outline => "outline",
            DocumentOperation::Body => "body",
            _ => "rewrite",
        };
        tx.execute("INSERT INTO document_jobs(request_id,document_id,created_ms,phase) VALUES(?1,?2,?3,?4)",params![request,id,now_ms(),phase]).map_err(storage)?;
        tx.commit().map_err(storage)
    }
    pub fn commit(
        &self,
        expected: &str,
        mut revision: DocumentRevision,
        request: Option<&str>,
    ) -> DocumentResult<DocumentRevision> {
        let mut db = self.connect()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        let (current,sequence):(String,u64)=tx.query_row("SELECT current_revision_id,(SELECT max(sequence) FROM document_revisions WHERE document_id=documents.id) FROM documents WHERE id=?1",[&revision.document_id],|row|Ok((row.get(0)?,row.get(1)?))).optional().map_err(storage)?.ok_or_else(||DocumentError::new(DocumentErrorCode::NotFound))?;
        if current != expected {
            return Err(DocumentError::new(DocumentErrorCode::Conflict));
        }
        if sequence >= 10_000 {
            return Err(DocumentError::new(DocumentErrorCode::Limit));
        }
        if let Some(request) = request {
            let phase: Option<String> = tx
                .query_row(
                    "SELECT phase FROM document_jobs WHERE request_id=?1 AND document_id=?2",
                    params![request, revision.document_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(storage)?;
            if !phase.is_some_and(|phase| matches!(phase.as_str(), "outline" | "body" | "rewrite"))
            {
                return Err(DocumentError::new(DocumentErrorCode::Cancelled));
            }
        }
        revision.id = fresh_id()?;
        revision.parent_revision_id = Some(expected.into());
        revision.sequence = sequence + 1;
        revision.created_ms = now_ms();
        revision.claims_verified = false;
        for block in &mut revision.blocks {
            block.claims_verified = false;
        }
        tx.execute(
            "INSERT INTO document_revisions(id,document_id,sequence,payload) VALUES(?1,?2,?3,?4)",
            params![
                revision.id,
                revision.document_id,
                revision.sequence,
                encode(&revision)?
            ],
        )
        .map_err(storage)?;
        tx.execute(
            "UPDATE documents SET current_revision_id=?1,updated_ms=?2 WHERE id=?3",
            params![revision.id, revision.created_ms, revision.document_id],
        )
        .map_err(storage)?;
        if let Some(request) = request {
            tx.execute(
                "UPDATE document_jobs SET phase='complete',error=NULL WHERE request_id=?1",
                [request],
            )
            .map_err(storage)?;
        }
        tx.commit().map_err(storage)?;
        Ok(revision)
    }
    pub fn fail_if_running(
        &self,
        request: &str,
        phase: &str,
        error: Option<DocumentError>,
    ) -> DocumentResult<()> {
        let error = error
            .map(|mut error| {
                error.message = DocumentError::new(error.code).message;
                if let Some(rag) = error.rag {
                    error.rag = Some(RagError::new(rag.code));
                }
                serde_json::to_string(&error)
            })
            .transpose()
            .map_err(storage)?;
        self.connect()?.execute("UPDATE document_jobs SET phase=?1,error=?2 WHERE request_id=?3 AND phase IN ('outline','body','rewrite')",params![phase,error,request]).map_err(storage)?;
        Ok(())
    }
    pub fn job(&self, id: &str) -> DocumentResult<Option<DocumentJob>> {
        let db = self.connect()?;
        let row:Option<(String,String,Option<String>)>=db.query_row("SELECT request_id,phase,error FROM document_jobs WHERE document_id=?1 ORDER BY created_ms DESC,rowid DESC LIMIT 1",[id],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional().map_err(storage)?;
        row.map(|(request_id, phase, error)| {
            let error = error
                .map(|value| serde_json::from_str::<DocumentError>(&value).map_err(storage))
                .transpose()?
                .map(|error| {
                    let mut clean = DocumentError::new(error.code);
                    clean.rag = error.rag.map(|value| RagError::new(value.code));
                    clean
                });
            Ok(DocumentJob {
                document_id: id.into(),
                request_id,
                phase,
                rag: None,
                error,
            })
        })
        .transpose()
    }
}
fn read_summary(row: &rusqlite::Row<'_>) -> rusqlite::Result<DocumentSummary> {
    Ok(DocumentSummary {
        id: row.get(0)?,
        title: row.get(1)?,
        created_ms: row.get(2)?,
        updated_ms: row.get(3)?,
        current_revision_id: row.get(4)?,
        revision_count: row.get(5)?,
        has_reviewed_outline: false,
    })
}
