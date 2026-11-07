//! Search-engine adapters for the independently testable RAG core.
//! Keep the producer's index lock and the RAG store's transaction intact:
//! no IPC array, result cap, or broader content permission is introduced here.
use crate::rag_service::{ConversationSummary, RagError, RagErrorCode, RagResult, RagService};
use std::sync::atomic::{AtomicBool, Ordering};

impl RagService {
    pub(crate) fn create_filename_search(
        &self,
        index: &crate::index::FileIndex,
        search: &crate::index::FilenameSearch,
        keyboard: bool,
        cancel: &AtomicBool,
    ) -> RagResult<ConversationSummary> {
        self.create_search_snapshot(&search.query, |visit| {
            index
                .snapshot_files(search, keyboard, cancel, visit)
                .map_err(|error| match error {
                    crate::index::SnapshotError::Visitor(error) => error,
                    crate::index::SnapshotError::Index(_) if cancel.load(Ordering::Acquire) => {
                        RagError::new(RagErrorCode::Cancelled)
                    }
                    crate::index::SnapshotError::Index(reason) => {
                        drop(reason);
                        RagError::new(RagErrorCode::SourceChanged)
                    }
                })
        })
    }
    pub(crate) fn create_keyword_search(
        &self,
        content: &crate::content_service::ContentService,
        search: &crate::content_service::KeywordSearch,
        cancel: &AtomicBool,
    ) -> RagResult<ConversationSummary> {
        self.create_search_snapshot(&search.query, |visit| {
            content
                .snapshot_keyword_files(search, cancel, visit)
                .map_err(|failure| match failure {
                    crate::content_store::SnapshotError::Visitor(value) => value,
                    crate::content_store::SnapshotError::Source(value) => {
                        RagError::new(if value.code == "cancelled" {
                            RagErrorCode::Cancelled
                        } else if value.code == "input" {
                            RagErrorCode::Input
                        } else {
                            RagErrorCode::SourceChanged
                        })
                    }
                })
        })
    }
}
