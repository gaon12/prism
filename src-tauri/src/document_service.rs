//! Typed document revisions and explicit generation requests.
use crate::rag_service::{ProviderSlot, RagCitation, RagError, RagErrorCode, RagJob, ScopeSnapshot, SourceRef};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub type DocumentResult<T> = Result<T, DocumentError>;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DocumentErrorCode {
    Input,
    NotFound,
    Conflict,
    Busy,
    OutlineReviewRequired,
    Malformed,
    Limit,
    Storage,
    Cancelled,
    Rag,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentError {
    pub code: DocumentErrorCode,
    pub message: String,
    pub rag: Option<RagError>,
}
impl DocumentError {
    pub(crate) fn new(code: DocumentErrorCode) -> Self {
        let message = match code {
            DocumentErrorCode::Input => "Invalid document input.",
            DocumentErrorCode::NotFound => "Document or revision was not found.",
            DocumentErrorCode::Conflict => "The document changed. Review the current revision before trying again.",
            DocumentErrorCode::Busy => "A document operation is already running.",
            DocumentErrorCode::OutlineReviewRequired => "Review and save the outline before generating the body.",
            DocumentErrorCode::Malformed => "The model returned an invalid document structure. The previous revision was preserved.",
            DocumentErrorCode::Limit => "The document exceeds the supported size or count limit.",
            DocumentErrorCode::Storage => "Document storage could not be accessed.",
            DocumentErrorCode::Cancelled => "Document generation was cancelled. The previous revision was preserved.",
            DocumentErrorCode::Rag => "The generation policy or provider rejected the document request.",
        };
        Self {
            code,
            message: message.into(),
            rag: None,
        }
    }
}
impl From<RagError> for DocumentError {
    fn from(error: RagError) -> Self {
        if error.code == RagErrorCode::Cancelled {
            return Self::new(DocumentErrorCode::Cancelled);
        }
        let mut value = Self::new(DocumentErrorCode::Rag);
        value.rag = Some(RagError::new(error.code));
        value
    }
}
pub(crate) fn storage(_: impl std::fmt::Display) -> DocumentError {
    DocumentError::new(DocumentErrorCode::Storage)
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlanSection {
    pub id: String,
    pub heading: String,
    pub instruction: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DocumentPlan {
    pub title: String,
    pub sections: Vec<PlanSection>,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum DocumentBlockKind {
    Fact,
    UserInput,
    AiProposal,
    Blank,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DocumentBlock {
    pub id: String,
    pub section_id: Option<String>,
    pub kind: DocumentBlockKind,
    pub markdown: String,
    pub source_labels: Vec<String>,
    pub claims_verified: bool,
    pub user_edited: bool,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum DocumentOperation {
    Create,
    Outline,
    OutlineEdit,
    Body,
    Rewrite,
    Manual,
    Restore,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentSummary {
    pub id: String,
    pub title: String,
    pub created_ms: u64,
    pub updated_ms: u64,
    pub current_revision_id: String,
    pub revision_count: u64,
    pub has_reviewed_outline: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentRevision {
    pub id: String,
    pub document_id: String,
    pub sequence: u64,
    pub parent_revision_id: Option<String>,
    pub created_ms: u64,
    pub operation: DocumentOperation,
    pub plan: DocumentPlan,
    pub outline_reviewed: bool,
    pub blocks: Vec<DocumentBlock>,
    pub markdown: String,
    pub scope_ids: Vec<String>,
    pub scope_snapshot: ScopeSnapshot,
    pub citations: Vec<RagCitation>,
    pub provenance: Vec<SourceRef>,
    pub external_input_eligible: bool,
    pub rag_request_id: Option<String>,
    pub provider: Option<ProviderSlot>,
    pub requested_model: Option<String>,
    pub reported_model: Option<String>,
    pub usage: Option<Value>,
    pub claims_verified: bool,
}
impl std::fmt::Debug for DocumentRevision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocumentRevision")
            .field("id", &self.id)
            .field("sequence", &self.sequence)
            .field("operation", &self.operation)
            .finish_non_exhaustive()
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentPage {
    pub document: DocumentSummary,
    pub revision: DocumentRevision,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentRevisionSummary {
    pub id: String,
    pub sequence: u64,
    pub parent_revision_id: Option<String>,
    pub created_ms: u64,
    pub operation: DocumentOperation,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentJob {
    pub document_id: String,
    pub request_id: String,
    pub phase: String,
    pub rag: Option<RagJob>,
    pub error: Option<DocumentError>,
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateDocument {
    pub title: String,
    pub purpose: String,
    #[serde(default)]
    pub scope_ids: Vec<String>,
    pub template_markdown: Option<String>,
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DocumentGenerateRequest {
    pub document_id: String,
    pub expected_revision_id: String,
    pub request_id: String,
    pub instruction: String,
    #[serde(default)]
    pub scope_ids: Vec<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DocumentSelection {
    pub block_id: String,
    pub start: usize,
    pub end: usize,
}
#[derive(Clone)]
pub struct DocumentRewriteRequest {
    pub base: DocumentGenerateRequest,
    pub target_block_ids: Vec<String>,
    pub selection: Option<DocumentSelection>,
}
impl<'de> Deserialize<'de> for DocumentRewriteRequest {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Keep the wire request flat without combining serde flatten with
        // deny_unknown_fields, which cannot reliably enforce this boundary.
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Wire {
            document_id: String,
            expected_revision_id: String,
            request_id: String,
            instruction: String,
            #[serde(default)]
            scope_ids: Vec<String>,
            target_block_ids: Vec<String>,
            selection: Option<DocumentSelection>,
        }
        let value = Wire::deserialize(deserializer)?;
        Ok(Self {
            base: DocumentGenerateRequest {
                document_id: value.document_id,
                expected_revision_id: value.expected_revision_id,
                request_id: value.request_id,
                instruction: value.instruction,
                scope_ids: value.scope_ids,
            },
            target_block_ids: value.target_block_ids,
            selection: value.selection,
        })
    }
}
