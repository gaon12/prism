//! SQLite conversation persistence. Provider prompts and retrieved raw document
//! chunks are never stored here; persisted text is the user's question/reply.
use crate::rag_service::{
    ConversationPage, ConversationSummary, ConversationTurn, EvidenceStatus, RagError,
    RagErrorCode, RagRequest, RagResult, ScopeSnapshot, TurnRole, TurnStatus,
};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const MAX_PAYLOAD: usize = 16 * 1024 * 1024;
const PAGE_BUDGET: usize = 8 * 1024 * 1024;
#[derive(Clone)]
pub(crate) struct RagStore {
    path: PathBuf,
    profile_id: String,
}
pub(crate) struct PendingPair {
    pub user: ConversationTurn,
    pub assistant: ConversationTurn,
}
fn storage(_: impl std::fmt::Display) -> RagError {
    RagError::new(RagErrorCode::Storage)
}
pub(crate) fn fresh_id() -> RagResult<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(storage)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}
pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |value| value.as_millis().min(i64::MAX as u128) as u64)
}
fn status_key(status: TurnStatus) -> &'static str {
    match status {
        TurnStatus::Pending => "pending",
        TurnStatus::Complete => "complete",
        TurnStatus::Cancelled => "cancelled",
        TurnStatus::Failed => "failed",
        TurnStatus::NoSources => "noSources",
        TurnStatus::Interrupted => "interrupted",
    }
}
fn decode(payload: String) -> RagResult<ConversationTurn> {
    if payload.len() > MAX_PAYLOAD {
        return Err(RagError::new(RagErrorCode::Limit));
    }
    let mut turn: ConversationTurn = serde_json::from_str(&payload).map_err(storage)?;
    turn.claims_verified = false;
    Ok(turn)
}
fn encode(turn: &ConversationTurn) -> RagResult<String> {
    let value = serde_json::to_string(turn).map_err(storage)?;
    if value.len() > MAX_PAYLOAD {
        return Err(RagError::new(RagErrorCode::Limit));
    }
    Ok(value)
}

impl RagStore {
    pub fn open(directory: &Path) -> RagResult<Self> {
        fs::create_dir_all(directory).map_err(storage)?;
        let path = directory.join("conversations.sqlite3");
        let mut store = Self {
            path,
            profile_id: String::new(),
        };
        let mut db = store.connect()?;
        let version: u32 = db
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(storage)?;
        if version > 4 {
            return Err(RagError::new(RagErrorCode::Storage));
        }
        db.execute_batch("PRAGMA journal_mode=WAL;
            CREATE TABLE IF NOT EXISTS rag_meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS conversations(id TEXT PRIMARY KEY,title TEXT NOT NULL,created_ms INTEGER NOT NULL,updated_ms INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS rag_requests(id TEXT PRIMARY KEY,conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,status TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS conversation_turns(id TEXT PRIMARY KEY,conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,request_id TEXT NOT NULL REFERENCES rag_requests(id) ON DELETE CASCADE,sequence INTEGER NOT NULL,role TEXT NOT NULL,status TEXT NOT NULL,text TEXT NOT NULL,payload TEXT NOT NULL,UNIQUE(conversation_id,sequence),UNIQUE(request_id,role));
            CREATE INDEX IF NOT EXISTS conversation_turns_order ON conversation_turns(conversation_id,sequence);
            ").map_err(storage)?;
        let has_kind = {
            let mut query = db
                .prepare("PRAGMA table_info(conversations)")
                .map_err(storage)?;
            let columns = query
                .query_map([], |row| row.get::<_, String>(1))
                .map_err(storage)?;
            columns
                .collect::<Result<Vec<_>, _>>()
                .map_err(storage)?
                .iter()
                .any(|name| name == "kind")
        };
        if !has_kind {
            db.execute(
                "ALTER TABLE conversations ADD COLUMN kind TEXT NOT NULL DEFAULT 'chat'",
                [],
            )
            .map_err(storage)?;
        }
        let has_project = {
            let mut query = db
                .prepare("PRAGMA table_info(conversations)")
                .map_err(storage)?;
            let columns = query
                .query_map([], |row| row.get::<_, String>(1))
                .map_err(storage)?;
            columns
                .collect::<Result<Vec<_>, _>>()
                .map_err(storage)?
                .iter()
                .any(|name| name == "project")
        };
        if !has_project {
            // Requirement Q37: the project (library folder) a conversation belongs to.
            // NULL means "no project"; such conversations appear only in the full list.
            db.execute("ALTER TABLE conversations ADD COLUMN project TEXT", [])
                .map_err(storage)?;
        }
        db.execute_batch("CREATE TABLE IF NOT EXISTS conversation_selection(conversation_id TEXT PRIMARY KEY REFERENCES conversations(id) ON DELETE CASCADE,payload TEXT NOT NULL);").map_err(storage)?;
        crate::selection_set::initialize(&db)?;
        db.pragma_update(None, "user_version", 4).map_err(storage)?;
        db.execute(
            "INSERT OR IGNORE INTO rag_meta(key,value) VALUES('profileId',?1)",
            [fresh_id()?],
        )
        .map_err(storage)?;
        store.profile_id = db
            .query_row(
                "SELECT value FROM rag_meta WHERE key='profileId'",
                [],
                |row| row.get(0),
            )
            .map_err(storage)?;
        if store.profile_id.len() != 32
            || !store
                .profile_id
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(RagError::new(RagErrorCode::Storage));
        }
        // A previous process cannot finish its pending network calls after restart.
        let pending: Vec<String> = {
            let mut query = db
                .prepare("SELECT payload FROM conversation_turns WHERE status='pending'")
                .map_err(storage)?;
            let rows = query.query_map([], |row| row.get(0)).map_err(storage)?;
            rows.collect::<Result<_, _>>().map_err(storage)?
        };
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        for payload in pending {
            let mut turn = decode(payload)?;
            turn.status = TurnStatus::Interrupted;
            turn.error = Some(RagError::new(RagErrorCode::Worker));
            tx.execute(
                "UPDATE conversation_turns SET status='interrupted',payload=?1 WHERE id=?2",
                params![encode(&turn)?, turn.id],
            )
            .map_err(storage)?;
        }
        tx.execute(
            "UPDATE rag_requests SET status='interrupted' WHERE status='pending'",
            [],
        )
        .map_err(storage)?;
        tx.commit().map_err(storage)?;
        Ok(store)
    }
    pub(crate) fn connect(&self) -> RagResult<Connection> {
        let db = Connection::open(&self.path).map_err(storage)?;
        db.busy_timeout(Duration::from_secs(3)).map_err(storage)?;
        db.execute_batch("PRAGMA foreign_keys=ON;")
            .map_err(storage)?;
        Ok(db)
    }
    pub fn profile_id(&self) -> &str {
        &self.profile_id
    }
    pub fn create(&self, title: &str, project: Option<&str>) -> RagResult<ConversationSummary> {
        let mut summary = self.create_kind(title, "chat", None)?;
        if let Some(project) = project {
            self.adopt_project(&summary.id, project)?;
            summary.project = Some(project.into());
        }
        Ok(summary)
    }
    /// Puts a conversation without a project into `project`. A conversation that already
    /// belongs to a project keeps it, so opening it from another project never moves it.
    pub fn adopt_project(&self, id: &str, project: &str) -> RagResult<()> {
        self.connect()?
            .execute(
                "UPDATE conversations SET project=?1 WHERE id=?2 AND kind='chat' AND project IS NULL",
                params![project, id],
            )
            .map_err(storage)?;
        Ok(())
    }
    pub fn create_search(
        &self,
        title: &str,
        selection: &crate::chat_selection::ChatSelection,
    ) -> RagResult<ConversationSummary> {
        self.create_kind(title, "chat", Some(selection))
    }
    pub fn create_for_document(&self, title: &str) -> RagResult<ConversationSummary> {
        self.create_kind(title, "document", None)
    }
    fn create_kind(
        &self,
        title: &str,
        kind: &str,
        selection: Option<&crate::chat_selection::ChatSelection>,
    ) -> RagResult<ConversationSummary> {
        let id = fresh_id()?;
        let now = now_ms();
        let mut db = self.connect()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        tx.execute(
            "INSERT INTO conversations(id,title,created_ms,updated_ms,kind) VALUES(?1,?2,?3,?3,?4)",
            params![id, title, now, kind],
        )
        .map_err(storage)?;
        if let Some(selection) = selection {
            tx.execute(
                "INSERT INTO conversation_selection(conversation_id,payload) VALUES(?1,?2)",
                params![id, serde_json::to_string(selection).map_err(storage)?],
            )
            .map_err(storage)?;
        }
        tx.commit().map_err(storage)?;
        Ok(ConversationSummary {
            id,
            title: title.into(),
            created_ms: now,
            updated_ms: now,
            turn_count: 0,
            project: None,
        })
    }
    /// Chat conversations, newest first. With `project`, only that project's conversations;
    /// without it, every conversation (each one shows its project, if any).
    pub fn list(
        &self,
        query: &str,
        limit: usize,
        project: Option<&str>,
    ) -> RagResult<Vec<ConversationSummary>> {
        let db = self.connect()?;
        let mut statement=db.prepare("SELECT c.id,c.title,c.created_ms,c.updated_ms,(SELECT count(*) FROM conversation_turns t WHERE t.conversation_id=c.id),c.project FROM conversations c WHERE c.kind='chat' AND (?3 IS NULL OR c.project=?3) AND (?1='' OR instr(lower(c.title),lower(?1))>0 OR EXISTS(SELECT 1 FROM conversation_turns t WHERE t.conversation_id=c.id AND instr(lower(t.text),lower(?1))>0)) ORDER BY c.updated_ms DESC,c.id LIMIT ?2").map_err(storage)?;
        let rows = statement
            .query_map(params![query, limit as u64, project], read_summary)
            .map_err(storage)?;
        rows.collect::<Result<_, _>>().map_err(storage)
    }
    pub fn selection(&self, id: &str) -> RagResult<Option<crate::chat_selection::ChatSelection>> {
        let payload: Option<String> = self
            .connect()?
            .query_row(
                "SELECT payload FROM conversation_selection WHERE conversation_id=?1",
                [id],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)?;
        payload
            .map(|payload| {
                if payload.len() > 4 * 1024 * 1024 {
                    return Err(RagError::new(RagErrorCode::Storage));
                }
                let selection = serde_json::from_str(&payload).map_err(storage)?;
                crate::chat_selection::validate(&selection).map_err(storage)?;
                Ok(selection)
            })
            .transpose()
    }
    pub fn set_selection(
        &self,
        id: &str,
        selection: Option<&crate::chat_selection::ChatSelection>,
    ) -> RagResult<()> {
        self.summary(id)?;
        if self.saved_selection(id)?.is_some() {
            return Err(RagError::new(RagErrorCode::Input));
        }
        let db = self.connect()?;
        if let Some(selection) = selection {
            db.execute("INSERT INTO conversation_selection(conversation_id,payload) VALUES(?1,?2) ON CONFLICT(conversation_id) DO UPDATE SET payload=excluded.payload", params![id,serde_json::to_string(selection).map_err(storage)?]).map_err(storage)?;
        } else {
            db.execute(
                "DELETE FROM conversation_selection WHERE conversation_id=?1",
                [id],
            )
            .map_err(storage)?;
        }
        Ok(())
    }
    pub fn summary(&self, id: &str) -> RagResult<ConversationSummary> {
        self.connect()?.query_row("SELECT c.id,c.title,c.created_ms,c.updated_ms,(SELECT count(*) FROM conversation_turns t WHERE t.conversation_id=c.id),c.project FROM conversations c WHERE c.id=?1",[id],read_summary).optional().map_err(storage)?.ok_or_else(||RagError::new(RagErrorCode::Input))
    }
    pub fn rename(&self, id: &str, title: &str) -> RagResult<()> {
        let count = self
            .connect()?
            .execute(
                "UPDATE conversations SET title=?1,updated_ms=?2 WHERE id=?3",
                params![title, now_ms(), id],
            )
            .map_err(storage)?;
        if count == 0 {
            return Err(RagError::new(RagErrorCode::Input));
        }
        Ok(())
    }
    pub fn delete(&self, id: &str) -> RagResult<()> {
        self.connect()?
            .execute("DELETE FROM conversations WHERE id=?1", [id])
            .map_err(storage)?;
        Ok(())
    }
    pub fn page(&self, id: &str, before: Option<u64>, limit: usize) -> RagResult<ConversationPage> {
        let conversation = self.summary(id)?;
        let db = self.connect()?;
        let mut query=db.prepare("SELECT payload FROM conversation_turns WHERE conversation_id=?1 AND sequence<?2 ORDER BY sequence DESC LIMIT ?3").map_err(storage)?;
        let mut rows = query
            .query(params![
                id,
                before.unwrap_or(i64::MAX as u64),
                limit as u64 + 1
            ])
            .map_err(storage)?;
        let mut turns = Vec::new();
        let mut bytes = 0usize;
        let mut has_more = false;
        while let Some(row) = rows.next().map_err(storage)? {
            let payload: String = row.get(0).map_err(storage)?;
            if turns.len() >= limit || (!turns.is_empty() && bytes + payload.len() > PAGE_BUDGET) {
                has_more = true;
                break;
            }
            bytes += payload.len();
            turns.push(decode(payload)?);
        }
        turns.reverse();
        Ok(ConversationPage {
            selection_set: self.selection_page(id, 0, 50)?,
            selection: self.selection(id)?,
            conversation,
            turns,
            has_more,
        })
    }
    pub fn reserve(&self, request: &RagRequest, snapshot: ScopeSnapshot) -> RagResult<PendingPair> {
        self.summary(&request.conversation_id)?;
        let mut db = self.connect()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        let exists: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM rag_requests WHERE id=?1)",
                [&request.request_id],
                |row| row.get(0),
            )
            .map_err(storage)?;
        if exists {
            return Err(RagError::new(RagErrorCode::DuplicateRequest));
        }
        let sequence:u64=tx.query_row("SELECT COALESCE(MAX(sequence),0)+1 FROM conversation_turns WHERE conversation_id=?1",[&request.conversation_id],|row|row.get(0)).map_err(storage)?;
        if sequence > 10_000 {
            return Err(RagError::new(RagErrorCode::Limit));
        }
        tx.execute(
            "INSERT INTO rag_requests(id,conversation_id,status) VALUES(?1,?2,'pending')",
            params![request.request_id, request.conversation_id],
        )
        .map_err(storage)?;
        let user = blank_turn(request, snapshot.clone(), TurnRole::User, sequence)?;
        let assistant = blank_turn(request, snapshot, TurnRole::Assistant, sequence + 1)?;
        for turn in [&user, &assistant] {
            tx.execute("INSERT INTO conversation_turns(id,conversation_id,request_id,sequence,role,status,text,payload) VALUES(?1,?2,?3,?4,?5,'pending',?6,?7)",params![turn.id,turn.conversation_id,turn.request_id,turn.sequence,if turn.role==TurnRole::User{"user"}else{"assistant"},turn.text,encode(turn)?]).map_err(storage)?;
        }
        tx.execute(
            "UPDATE conversations SET updated_ms=?1 WHERE id=?2",
            params![now_ms(), request.conversation_id],
        )
        .map_err(storage)?;
        tx.commit().map_err(storage)?;
        Ok(PendingPair { user, assistant })
    }
    pub fn history(
        &self,
        id: &str,
        limit: usize,
    ) -> RagResult<Vec<(ConversationTurn, ConversationTurn)>> {
        let db = self.connect()?;
        let mut query=db.prepare("SELECT u.payload,a.payload FROM rag_requests r JOIN conversation_turns u ON u.request_id=r.id AND u.role='user' JOIN conversation_turns a ON a.request_id=r.id AND a.role='assistant' WHERE r.conversation_id=?1 AND r.status='complete' ORDER BY a.sequence DESC LIMIT ?2").map_err(storage)?;
        let rows = query
            .query_map(params![id, limit as u64], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(storage)?;
        let mut result = Vec::new();
        for row in rows {
            let (user, assistant) = row.map_err(storage)?;
            result.push((decode(user)?, decode(assistant)?));
        }
        Ok(result)
    }
    pub fn update_pending_user(&self, user: &ConversationTurn) -> RagResult<()> {
        self.connect()?
            .execute(
                "UPDATE conversation_turns SET payload=?1 WHERE id=?2 AND status='pending'",
                params![encode(user)?, user.id],
            )
            .map_err(storage)?;
        Ok(())
    }
    pub fn finish(&self, user: &ConversationTurn, assistant: &ConversationTurn) -> RagResult<()> {
        let mut db = self.connect()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        let pending: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM rag_requests WHERE id=?1 AND status='pending')",
                [&user.request_id],
                |row| row.get(0),
            )
            .map_err(storage)?;
        if !pending {
            return Err(RagError::new(RagErrorCode::DuplicateRequest));
        }
        for turn in [user, assistant] {
            tx.execute(
                "UPDATE conversation_turns SET status=?1,text=?2,payload=?3 WHERE id=?4",
                params![status_key(turn.status), turn.text, encode(turn)?, turn.id],
            )
            .map_err(storage)?;
        }
        tx.execute(
            "UPDATE rag_requests SET status=?1 WHERE id=?2",
            params![status_key(assistant.status), user.request_id],
        )
        .map_err(storage)?;
        tx.execute(
            "UPDATE conversations SET updated_ms=?1 WHERE id=?2",
            params![now_ms(), user.conversation_id],
        )
        .map_err(storage)?;
        tx.commit().map_err(storage)?;
        Ok(())
    }
    pub fn set_sources_hidden(&self, turn_id: &str, hidden: bool) -> RagResult<()> {
        let db = self.connect()?;
        let payload: Option<String> = db
            .query_row(
                "SELECT payload FROM conversation_turns WHERE id=?1",
                [turn_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)?;
        let mut turn = decode(payload.ok_or_else(|| RagError::new(RagErrorCode::Input))?)?;
        turn.sources_hidden = hidden;
        db.execute(
            "UPDATE conversation_turns SET payload=?1 WHERE id=?2",
            params![encode(&turn)?, turn_id],
        )
        .map_err(storage)?;
        Ok(())
    }
    pub fn all_turns(&self, id: &str) -> RagResult<Vec<ConversationTurn>> {
        self.summary(id)?;
        let db = self.connect()?;
        let mut query = db
            .prepare(
                "SELECT payload FROM conversation_turns WHERE conversation_id=?1 ORDER BY sequence",
            )
            .map_err(storage)?;
        let mut rows = query.query([id]).map_err(storage)?;
        let mut result = Vec::new();
        let mut bytes = 0usize;
        while let Some(row) = rows.next().map_err(storage)? {
            let payload: String = row.get(0).map_err(storage)?;
            bytes += payload.len();
            if bytes > 16 * 1024 * 1024 {
                return Err(RagError::new(RagErrorCode::Limit));
            }
            result.push(decode(payload)?);
        }
        Ok(result)
    }
}
fn read_summary(row: &rusqlite::Row<'_>) -> rusqlite::Result<ConversationSummary> {
    Ok(ConversationSummary {
        id: row.get(0)?,
        title: row.get(1)?,
        created_ms: row.get(2)?,
        updated_ms: row.get(3)?,
        turn_count: row.get(4)?,
        project: row.get(5)?,
    })
}
fn blank_turn(
    request: &RagRequest,
    snapshot: ScopeSnapshot,
    role: TurnRole,
    sequence: u64,
) -> RagResult<ConversationTurn> {
    Ok(ConversationTurn {
        id: fresh_id()?,
        conversation_id: request.conversation_id.clone(),
        request_id: request.request_id.clone(),
        sequence,
        role,
        text: if role == TurnRole::User {
            request.question.clone()
        } else {
            String::new()
        },
        created_ms: now_ms(),
        status: TurnStatus::Pending,
        scope_snapshot: snapshot,
        sources_hidden: request.sources_hidden,
        citations: vec![],
        provenance: vec![],
        taint_complete: false,
        evidence: EvidenceStatus::NoSources,
        claims_verified: false,
        unknown_citations: vec![],
        provider: None,
        requested_model: None,
        reported_model: None,
        finish_reason: None,
        usage: None,
        error: None,
    })
}
#[cfg(test)]
mod selection_tests {
    use super::*;
    #[test]
    fn version_two_conversations_migrate_and_damaged_selections_fail_closed() {
        let directory = tempfile::tempdir().unwrap();
        let store = RagStore::open(directory.path()).unwrap();
        let conversation = store.create("legacy conversation", None).unwrap();
        let db = store.connect().unwrap();
        db.execute_batch("DROP TABLE conversation_selection; PRAGMA user_version=2;")
            .unwrap();
        drop(db);
        let store = RagStore::open(directory.path()).unwrap();
        assert_eq!(
            store.summary(&conversation.id).unwrap().title,
            "legacy conversation"
        );
        assert!(store.selection(&conversation.id).unwrap().is_none());
        let selection = crate::chat_selection::ChatSelection {
            query: "fixture".into(),
            documents: vec!["source.md".into()],
            selected_documents: vec![],
            partial: false,
        };
        store
            .set_selection(&conversation.id, Some(&selection))
            .unwrap();
        assert!(store
            .selection(&conversation.id)
            .unwrap()
            .unwrap()
            .selected_documents
            .is_empty());
        store
            .connect()
            .unwrap()
            .execute(
                "UPDATE conversation_selection SET payload='damaged' WHERE conversation_id=?1",
                [&conversation.id],
            )
            .unwrap();
        assert_eq!(
            store.selection(&conversation.id).err().unwrap().code,
            RagErrorCode::Storage
        );
        store
            .connect()
            .unwrap()
            .execute_batch("PRAGMA user_version=5;")
            .unwrap();
        assert_eq!(
            RagStore::open(directory.path()).err().unwrap().code,
            RagErrorCode::Storage
        );
    }

    #[test]
    fn conversations_are_listed_per_project_and_keep_their_first_project() {
        let directory = tempfile::tempdir().unwrap();
        let store = RagStore::open(directory.path()).unwrap();
        let a = store.create("in A", Some("project-a")).unwrap();
        assert_eq!(a.project.as_deref(), Some("project-a"));
        let loose = store.create("no project", None).unwrap();

        // A project shows only its own conversations; no project shows everything.
        let ids = |project| -> Vec<String> {
            store
                .list("", 100, project)
                .unwrap()
                .into_iter()
                .map(|item| item.id)
                .collect()
        };
        assert_eq!(ids(Some("project-a")), [a.id.clone()]);
        assert!(ids(Some("project-b")).is_empty());
        assert_eq!(ids(None).len(), 2);

        // A search handed over inside project B files the loose conversation there, but a
        // conversation that already has a project is never moved.
        store.adopt_project(&loose.id, "project-b").unwrap();
        store.adopt_project(&a.id, "project-b").unwrap();
        assert_eq!(ids(Some("project-b")), [loose.id.clone()]);
        assert_eq!(
            store.summary(&a.id).unwrap().project.as_deref(),
            Some("project-a")
        );

        // Reopening keeps the column (the migration only adds it once).
        let reopened = RagStore::open(directory.path()).unwrap();
        assert_eq!(reopened.list("", 100, Some("project-a")).unwrap().len(), 1);
    }
}
