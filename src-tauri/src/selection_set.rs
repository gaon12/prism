//! Large search snapshots live in rows, not in an IPC array. A saved set still
//! grants no content access: the resolver must apply current folder/hash policy.
use crate::{
    chat_selection::path_key,
    rag_service::{ConversationSummary, RagError, RagErrorCode, RagResult},
    rag_store::{fresh_id, now_ms, RagStore},
};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};

fn storage(_: impl std::fmt::Display) -> RagError {
    RagError::new(RagErrorCode::Storage)
}
pub(crate) fn initialize(db: &Connection) -> RagResult<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS search_file_sets(
      conversation_id TEXT PRIMARY KEY REFERENCES conversations(id) ON DELETE CASCADE,
      query TEXT NOT NULL,partial INTEGER NOT NULL,revision INTEGER NOT NULL);
      CREATE TABLE IF NOT EXISTS search_set_files(
      conversation_id TEXT NOT NULL REFERENCES search_file_sets(conversation_id) ON DELETE CASCADE,
      ordinal INTEGER NOT NULL,path_key TEXT NOT NULL,file_path TEXT NOT NULL,selected INTEGER NOT NULL,
      PRIMARY KEY(conversation_id,ordinal),UNIQUE(conversation_id,path_key));
      CREATE INDEX IF NOT EXISTS search_set_selected ON search_set_files(conversation_id,selected,ordinal);
      CREATE INDEX IF NOT EXISTS search_set_selected_path ON search_set_files(conversation_id,selected,path_key);")
      .map_err(storage)
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionFile {
    pub id: u64,
    pub path: String,
    pub selected: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionPage {
    pub query: String,
    pub partial: bool,
    pub revision: u64,
    pub total: u64,
    pub selected_count: u64,
    pub offset: u64,
    pub files: Vec<SelectionFile>,
    pub has_more: bool,
}
/// Private read handle. Holding an active conversation reservation makes edits
/// impossible; revision checks additionally fail closed if the DB changes.
#[derive(Clone)]
pub(crate) struct SavedSelection {
    store: RagStore,
    id: String,
    revision: u64,
}
impl SavedSelection {
    fn checked_connection(&self) -> RagResult<Connection> {
        let db = self.store.connect()?;
        let current: Option<u64> = db
            .query_row(
                "SELECT revision FROM search_file_sets WHERE conversation_id=?1",
                [&self.id],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)?;
        if current != Some(self.revision) {
            return Err(RagError::new(RagErrorCode::PolicyChanged));
        }
        Ok(db)
    }
    pub fn contains(&self, path: &str) -> RagResult<bool> {
        self.checked_connection()?.query_row("SELECT EXISTS(SELECT 1 FROM search_set_files WHERE conversation_id=?1 AND path_key=?2 AND selected=1)", params![self.id,path_key(path)], |row| row.get(0)).map_err(storage)
    }
    pub fn batch(&self, after: u64) -> RagResult<Vec<SelectionFile>> {
        self.read_batch(
            "ordinal",
            rusqlite::types::Value::Integer(i64::try_from(after).map_err(storage)?),
            None,
        )
    }
    /// A lexical root predicate is only a cheap candidate reduction. Files are
    /// still canonicalized, authorized and hash-checked by ContentSources.
    pub fn batch_in_roots(
        &self,
        after: &str,
        roots: Option<&[String]>,
    ) -> RagResult<Vec<SelectionFile>> {
        // Retrieval batches must not depend on the producer's visit order:
        // per-batch candidate limits could otherwise change the final hits.
        // The selected/path index streams this order without a full path Vec.
        self.read_batch(
            "path_key",
            rusqlite::types::Value::Text(after.to_owned()),
            roots,
        )
    }
    fn read_batch(
        &self,
        column: &'static str,
        after: rusqlite::types::Value,
        roots: Option<&[String]>,
    ) -> RagResult<Vec<SelectionFile>> {
        let db = self.checked_connection()?;
        let mut parameters = vec![rusqlite::types::Value::Text(self.id.clone()), after];
        let restriction = if let Some(roots) = roots {
            if roots.is_empty() {
                return Ok(vec![]);
            }
            if roots.len() > 128 {
                return Err(RagError::new(RagErrorCode::Limit));
            }
            let conditions: Vec<_> = roots
                .iter()
                .enumerate()
                .map(|(index, root)| {
                    parameters.push(rusqlite::types::Value::Text(root.clone()));
                    let p = index + 3;
                    format!("(path_key=?{p} OR substr(path_key,1,length(?{p})+1)=?{p}||'/')")
                })
                .collect();
            format!(" AND ({})", conditions.join(" OR "))
        } else {
            String::new()
        };
        let mut query = db.prepare(&format!("SELECT ordinal,file_path,selected FROM search_set_files WHERE conversation_id=?1 AND selected=1 AND {column}>?2{restriction} ORDER BY {column} LIMIT 256")).map_err(storage)?;
        let rows = query
            .query_map(rusqlite::params_from_iter(parameters), read_file)
            .map_err(storage)?;
        rows.collect::<Result<_, _>>().map_err(storage)
    }
}
fn read_file(row: &rusqlite::Row<'_>) -> rusqlite::Result<SelectionFile> {
    Ok(SelectionFile {
        id: row.get(0)?,
        path: row.get(1)?,
        selected: row.get(2)?,
    })
}
impl RagStore {
    /// The producer streams every file from one locked index revision into this
    /// transaction. A producer/SQLite failure publishes neither the set nor chat.
    pub(crate) fn create_file_set(
        &self,
        query: &str,
        produce: impl FnOnce(&mut dyn FnMut(&str) -> RagResult<()>) -> RagResult<bool>,
    ) -> RagResult<ConversationSummary> {
        if query.len() > 4096 || query.contains('\0') {
            return Err(RagError::new(RagErrorCode::Input));
        }
        let id = fresh_id()?;
        let now = now_ms();
        let title: String = query
            .chars()
            .filter(|c| !c.is_control())
            .take(200)
            .collect();
        let mut db = self.connect()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        tx.execute("INSERT INTO conversations(id,title,created_ms,updated_ms,kind) VALUES(?1,?2,?3,?3,'chat')", params![id,title,now]).map_err(storage)?;
        tx.execute("INSERT INTO search_file_sets(conversation_id,query,partial,revision) VALUES(?1,?2,0,1)",params![id,query]).map_err(storage)?;
        let mut ordinal = 0u64;
        let partial = {
            let mut insert = tx.prepare("INSERT OR IGNORE INTO search_set_files(conversation_id,ordinal,path_key,file_path,selected) VALUES(?1,?2,?3,?4,1)").map_err(storage)?;
            produce(&mut |path| {
                if path.is_empty() || path.len() > 32768 || path.chars().any(char::is_control) {
                    return Err(RagError::new(RagErrorCode::Input));
                }
                let next = ordinal
                    .checked_add(1)
                    .filter(|n| *n <= i64::MAX as u64)
                    .ok_or_else(|| RagError::new(RagErrorCode::Limit))?;
                if insert
                    .execute(params![id, next, path_key(path), path])
                    .map_err(storage)?
                    > 0
                {
                    ordinal = next;
                }
                Ok(())
            })?
        };
        tx.execute(
            "UPDATE search_file_sets SET partial=?1 WHERE conversation_id=?2",
            params![partial, id],
        )
        .map_err(storage)?;
        tx.commit().map_err(storage)?;
        Ok(ConversationSummary {
            id,
            title,
            created_ms: now,
            updated_ms: now,
            turn_count: 0,
            project: None,
        })
    }
    pub(crate) fn saved_selection(&self, id: &str) -> RagResult<Option<SavedSelection>> {
        let revision: Option<u64> = self
            .connect()?
            .query_row(
                "SELECT revision FROM search_file_sets WHERE conversation_id=?1",
                [id],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)?;
        Ok(revision.map(|revision| SavedSelection {
            store: self.clone(),
            id: id.into(),
            revision,
        }))
    }
    pub(crate) fn selection_page(
        &self,
        id: &str,
        offset: u64,
        limit: usize,
    ) -> RagResult<Option<SelectionPage>> {
        if !(1..=50).contains(&limit) || offset > i64::MAX as u64 {
            return Err(RagError::new(RagErrorCode::Input));
        }
        let mut db = self.connect()?;
        let tx = db.transaction().map_err(storage)?;
        let meta: Option<(String, bool, u64)> = tx
            .query_row(
                "SELECT query,partial,revision FROM search_file_sets WHERE conversation_id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(storage)?;
        let Some((query, partial, revision)) = meta else {
            return Ok(None);
        };
        let (total,selected_count): (u64,u64)=tx.query_row("SELECT count(*),COALESCE(sum(selected),0) FROM search_set_files WHERE conversation_id=?1", [id], |r| Ok((r.get(0)?,r.get(1)?))).map_err(storage)?;
        let mut statement=tx.prepare("SELECT ordinal,file_path,selected FROM search_set_files WHERE conversation_id=?1 ORDER BY ordinal LIMIT ?2 OFFSET ?3").map_err(storage)?;
        let files = statement
            .query_map(params![id, limit as u64, offset], read_file)
            .map_err(storage)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage)?;
        Ok(Some(SelectionPage {
            query,
            partial,
            revision,
            total,
            selected_count,
            offset,
            has_more: offset.saturating_add(files.len() as u64) < total,
            files,
        }))
    }
    pub(crate) fn edit_file_set(
        &self,
        id: &str,
        revision: u64,
        changes: &[(u64, bool)],
        all: Option<bool>,
    ) -> RagResult<()> {
        if changes.len() > 50 {
            return Err(RagError::new(RagErrorCode::Input));
        }
        let mut db = self.connect()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        let current: Option<u64> = tx
            .query_row(
                "SELECT revision FROM search_file_sets WHERE conversation_id=?1",
                [id],
                |r| r.get(0),
            )
            .optional()
            .map_err(storage)?;
        if current != Some(revision) {
            return Err(RagError::new(RagErrorCode::PolicyChanged));
        }
        if let Some(selected) = all {
            tx.execute(
                "UPDATE search_set_files SET selected=?1 WHERE conversation_id=?2",
                params![selected, id],
            )
            .map_err(storage)?;
        }
        {
            let mut seen = std::collections::HashSet::new();
            for (ordinal, selected) in changes {
                if !seen.insert(ordinal) || tx.execute("UPDATE search_set_files SET selected=?1 WHERE conversation_id=?2 AND ordinal=?3",params![selected,id,ordinal]).map_err(storage)? != 1 {return Err(RagError::new(RagErrorCode::Input));}
            }
        }
        tx.execute(
            "UPDATE search_file_sets SET revision=revision+1 WHERE conversation_id=?1",
            [id],
        )
        .map_err(storage)?;
        tx.commit().map_err(storage)
    }
    pub(crate) fn clear_file_set(&self, id: &str, revision: u64) -> RagResult<()> {
        let changed = self
            .connect()?
            .execute(
                "DELETE FROM search_file_sets WHERE conversation_id=?1 AND revision=?2",
                params![id, revision],
            )
            .map_err(storage)?;
        if changed != 1 {
            return Err(RagError::new(RagErrorCode::PolicyChanged));
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn path_batches(set: &SavedSelection, roots: &[String]) -> Vec<Vec<String>> {
        let mut result = Vec::new();
        let mut after = String::new();
        loop {
            let batch = set.batch_in_roots(&after, Some(roots)).unwrap();
            let Some(last) = batch.last() else { break };
            assert!(batch.len() <= 256);
            let next = path_key(&last.path);
            assert!(next > after);
            after = next;
            result.push(batch.into_iter().map(|file| path_key(&file.path)).collect());
        }
        result
    }
    #[test]
    fn retrieval_pages_are_identical_across_producer_orders_and_keep_ui_ordinals() {
        let dir = tempfile::tempdir().unwrap();
        let store = RagStore::open(dir.path()).unwrap();
        let paths: Vec<_> = (0..769)
            .map(|i| format!("/fixture/한글-{i:04}.md"))
            .collect();
        let orders = [
            (0..paths.len()).collect::<Vec<_>>(),
            (0..paths.len()).rev().collect(),
            (0..paths.len()).map(|i| (i * 37) % paths.len()).collect(),
        ];
        let roots = vec!["/fixture".into()];
        let mut expected: Vec<_> = paths
            .iter()
            .enumerate()
            .filter(|(i, _)| ![1, 256, 513].contains(i))
            .map(|(_, path)| path_key(path))
            .collect();
        expected.sort();
        let mut reference = None;
        for order in orders {
            let chat = store
                .create_file_set("same files", |emit| {
                    for &i in &order {
                        emit(&paths[i])?;
                        if i % 17 == 0 {
                            emit(&paths[i])?;
                        }
                    }
                    emit("/fixture-sibling/outside.md")?;
                    emit("/other/outside.md")?;
                    Ok(false)
                })
                .unwrap();
            let before = store.selection_page(&chat.id, 0, 50).unwrap().unwrap();
            assert_eq!(before.files[0].path, paths[order[0]]);
            let changes: Vec<_> = [1, 256, 513]
                .iter()
                .map(|&i| {
                    let ordinal = store
                        .connect()
                        .unwrap()
                        .query_row(
                            "SELECT ordinal FROM search_set_files WHERE conversation_id=?1 AND path_key=?2",
                            params![chat.id, path_key(&paths[i])],
                            |row| row.get(0),
                        )
                        .unwrap();
                    (ordinal, false)
                })
                .collect();
            store.edit_file_set(&chat.id, 1, &changes, None).unwrap();
            let set = store.saved_selection(&chat.id).unwrap().unwrap();
            let pages = path_batches(&set, &roots);
            assert_eq!(
                pages.iter().map(Vec::len).collect::<Vec<_>>(),
                [256, 256, 254]
            );
            assert_eq!(pages.concat(), expected);
            if let Some(reference) = &reference {
                assert_eq!(&pages, reference);
            } else {
                reference = Some(pages);
            }
            let after = store.selection_page(&chat.id, 0, 50).unwrap().unwrap();
            assert_eq!(after.files[0].id, before.files[0].id);
            assert_eq!(after.files[0].path, before.files[0].path);
            assert!(set.batch_in_roots("", Some(&[])).unwrap().is_empty());
            assert!(set
                .batch_in_roots("", Some(&vec!["/fixture".into(); 129]))
                .is_err());
            store.edit_file_set(&chat.id, 2, &[], Some(false)).unwrap();
            assert!(matches!(
                set.batch_in_roots("", Some(&roots)),
                Err(RagError {
                    code: RagErrorCode::PolicyChanged,
                    ..
                })
            ));
            let set = store.saved_selection(&chat.id).unwrap().unwrap();
            assert!(set.batch_in_roots("", Some(&roots)).unwrap().is_empty());
        }
    }
    #[test]
    fn full_sets_page_edit_reopen_and_rollback_without_ipc_sized_limits() {
        let dir = tempfile::tempdir().unwrap();
        let store = RagStore::open(dir.path()).unwrap();
        let chat = store
            .create_file_set("all matches", |emit| {
                for i in 0..1005 {
                    emit(&format!("/fixture/file-{i:04}.md"))?;
                }
                emit("/fixture/file-0000.md")?;
                Ok(false)
            })
            .unwrap();
        let first = store.selection_page(&chat.id, 0, 50).unwrap().unwrap();
        assert_eq!(first.total, 1005);
        assert_eq!(first.selected_count, 1005);
        assert_eq!(first.files.len(), 50);
        assert!(first.has_more);
        let last = store.selection_page(&chat.id, 1000, 50).unwrap().unwrap();
        assert_eq!(last.files.len(), 5);
        assert!(!last.has_more);
        let old = store.saved_selection(&chat.id).unwrap().unwrap();
        store
            .edit_file_set(&chat.id, 1, &[(1005, true)], Some(false))
            .unwrap();
        assert!(old.contains("/fixture/file-0000.md").is_err());
        let set = store.saved_selection(&chat.id).unwrap().unwrap();
        assert!(!set.contains("/fixture/file-0000.md").unwrap());
        assert!(set.contains("/fixture/file-1004.md").unwrap());
        assert_eq!(set.batch(0).unwrap()[0].id, 1005);
        assert!(set.batch(1005).unwrap().is_empty());
        assert!(store
            .edit_file_set(&chat.id, 2, &[(1005, false), (9999, true)], None)
            .is_err());
        assert_eq!(
            store
                .selection_page(&chat.id, 0, 50)
                .unwrap()
                .unwrap()
                .selected_count,
            1
        );
        assert!(store.clear_file_set(&chat.id, 1).is_err());
        let count = store.list("", 100, None).unwrap().len();
        assert!(store
            .create_file_set("cancelled", |emit| {
                emit("/fixture/partial.md")?;
                Err(RagError::new(RagErrorCode::Cancelled))
            })
            .is_err());
        assert_eq!(store.list("", 100, None).unwrap().len(), count);
        let reopened = RagStore::open(dir.path()).unwrap();
        assert_eq!(
            reopened
                .selection_page(&chat.id, 0, 50)
                .unwrap()
                .unwrap()
                .selected_count,
            1
        );
        reopened.delete(&chat.id).unwrap();
        assert!(reopened.saved_selection(&chat.id).unwrap().is_none());
        assert_eq!(
            reopened
                .connect()
                .unwrap()
                .query_row("SELECT count(*) FROM search_set_files", [], |r| r
                    .get::<_, u64>(0))
                .unwrap(),
            0
        );
    }
}
