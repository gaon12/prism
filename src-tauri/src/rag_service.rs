//! Document-grounded conversation orchestration. Policy and source authority are
//! injected by the application; no frontend IPC or raw filesystem access lives here.
use crate::{
    generation_runtime::{
        validate_endpoint, GenerationCancellation, GenerationConfig, GenerationError,
        GenerationErrorCode, GenerationMessage, GenerationResponse, GenerationRole,
        GenerationRuntime,
    },
    secret_store::{SecretSupport, SecretValue},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fmt,
    future::Future,
    pin::Pin,
    sync::{atomic::AtomicBool, Arc},
};

pub type RagResult<T> = Result<T, RagError>;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RagErrorCode {
    Input,
    Storage,
    Busy,
    DuplicateRequest,
    NotConfigured,
    NoSources,
    ScopeBlocked,
    PolicyChanged,
    SourceChanged,
    Cancelled,
    Network,
    Auth,
    RateLimited,
    Context,
    Server,
    Malformed,
    Endpoint,
    Worker,
    Unsupported,
    Limit,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RagError {
    pub code: RagErrorCode,
    pub message: String,
}
impl RagError {
    pub fn new(code: RagErrorCode) -> Self {
        let message = match code {
            RagErrorCode::Input => "Check the conversation, question, selected scopes, and generation settings.",
            RagErrorCode::Storage => "The conversation repository could not be accessed. Existing conversations were preserved.",
            RagErrorCode::Busy => "This conversation already has an active request.",
            RagErrorCode::DuplicateRequest => "This request was already recorded. Read the conversation or submit a new request identifier.",
            RagErrorCode::NotConfigured => "Configure the selected generation provider before asking a question.",
            RagErrorCode::NoSources => "No current document evidence was found in the selected scopes. No general answer was generated.",
            RagErrorCode::ScopeBlocked => "The current policy does not allow sending this question and document context to the external provider.",
            RagErrorCode::PolicyChanged => "Permissions or generation settings changed while preparing the request. Submit it again.",
            RagErrorCode::SourceChanged => "Document evidence changed or became unavailable while preparing the request.",
            RagErrorCode::Cancelled => "Generation cancelled.",
            RagErrorCode::Network => "The generation provider could not be reached or timed out.",
            RagErrorCode::Auth => "The endpoint-bound API key is missing or authentication failed.",
            RagErrorCode::RateLimited => "The generation provider is rate limiting requests.",
            RagErrorCode::Context => "The question, evidence, or history exceeds the input budget or provider context limit.",
            RagErrorCode::Server => "The generation provider could not complete the request.",
            RagErrorCode::Malformed => "The generation provider returned an invalid, unsupported, or oversized response.",
            RagErrorCode::Endpoint => "The selected provider endpoint is invalid or belongs to the wrong local/external slot.",
            RagErrorCode::Worker => "The conversation worker stopped unexpectedly.",
            RagErrorCode::Unsupported => "Secure credential storage is not supported on this operating system.",
            RagErrorCode::Limit => "This conversation operation exceeds its size or item limit.",
        };
        Self {
            code,
            message: message.into(),
        }
    }
    fn sanitized(self) -> Self {
        Self::new(self.code)
    }
}
impl fmt::Display for RagError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for RagError {}
impl From<GenerationError> for RagError {
    fn from(value: GenerationError) -> Self {
        Self::new(match value.code {
            GenerationErrorCode::Input => RagErrorCode::Input,
            GenerationErrorCode::Endpoint => RagErrorCode::Endpoint,
            GenerationErrorCode::Network => RagErrorCode::Network,
            GenerationErrorCode::Auth => RagErrorCode::Auth,
            GenerationErrorCode::RateLimited => RagErrorCode::RateLimited,
            GenerationErrorCode::Context => RagErrorCode::Context,
            GenerationErrorCode::Server => RagErrorCode::Server,
            GenerationErrorCode::Malformed => RagErrorCode::Malformed,
            GenerationErrorCode::Cancelled => RagErrorCode::Cancelled,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GenerationMode {
    #[default]
    Local,
    External,
    Auto,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderPriority {
    #[default]
    LocalFirst,
    ExternalFirst,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderSlot {
    Local,
    External,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GenerationPreferences {
    pub mode: GenerationMode,
    pub priority: ProviderPriority,
    pub fallback: bool,
    pub allow_external_questions: bool,
    pub max_context_chars: Option<usize>,
    pub top_k: Option<usize>,
    pub local: Option<GenerationConfig>,
    pub external: Option<GenerationConfig>,
}
impl Default for GenerationPreferences {
    fn default() -> Self {
        Self {
            mode: GenerationMode::Local,
            priority: ProviderPriority::LocalFirst,
            fallback: false,
            allow_external_questions: false,
            max_context_chars: None,
            top_k: None,
            local: None,
            external: None,
        }
    }
}
impl fmt::Debug for GenerationPreferences {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GenerationPreferences")
            .field("mode", &self.mode)
            .field("priority", &self.priority)
            .field("fallback", &self.fallback)
            .field("local_configured", &self.local.is_some())
            .field("external_configured", &self.external.is_some())
            .finish_non_exhaustive()
    }
}
impl GenerationPreferences {
    pub fn validate(&self) -> RagResult<()> {
        if self
            .max_context_chars
            .is_some_and(|value| !(512..=200_000).contains(&value))
            || self.top_k.is_some_and(|value| !(1..=32).contains(&value))
        {
            return Err(RagError::new(RagErrorCode::Input));
        }
        for (slot, config) in [
            (ProviderSlot::Local, self.local.as_ref()),
            (ProviderSlot::External, self.external.as_ref()),
        ] {
            if let Some(config) = config {
                slot_endpoint(slot, &config.endpoint)?;
                if config.model.trim().is_empty()
                    || config.model.len() > 256
                    || config.model.chars().any(char::is_control)
                    || config.max_tokens == 0
                    || config.max_tokens > 65_536
                    || config
                        .temperature
                        .is_some_and(|value| !value.is_finite() || !(0.0..=2.0).contains(&value))
                {
                    return Err(RagError::new(RagErrorCode::Input));
                }
            }
        }
        Ok(())
    }
    fn budget(&self) -> usize {
        self.max_context_chars.unwrap_or(8000)
    }
    fn top_k(&self) -> usize {
        self.top_k.unwrap_or(6)
    }
    fn config(&self, slot: ProviderSlot) -> Option<&GenerationConfig> {
        match slot {
            ProviderSlot::Local => self.local.as_ref(),
            ProviderSlot::External => self.external.as_ref(),
        }
    }
    fn plan(&self) -> Vec<ProviderSlot> {
        match self.mode {
            GenerationMode::Local => vec![ProviderSlot::Local],
            GenerationMode::External => vec![ProviderSlot::External],
            GenerationMode::Auto => {
                let first = if self.priority == ProviderPriority::LocalFirst {
                    ProviderSlot::Local
                } else {
                    ProviderSlot::External
                };
                let other = if first == ProviderSlot::Local {
                    ProviderSlot::External
                } else {
                    ProviderSlot::Local
                };
                if self.fallback {
                    vec![first, other]
                } else {
                    vec![first]
                }
            }
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RagScope {
    pub id: String,
    pub name: String,
    pub path: String,
    pub content_enabled: bool,
    pub external_allowed: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RagPolicy {
    pub revision: u64,
    pub prefs: GenerationPreferences,
    pub scopes: Vec<RagScope>,
}
pub type PolicyGetter = Arc<dyn Fn() -> RagResult<RagPolicy> + Send + Sync>;

#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceRef {
    pub chunk_id: String,
    pub scope_id: String,
    pub file_path: String,
    pub sha256: String,
    pub page: Option<u32>,
    pub paragraph: Option<u32>,
}
impl fmt::Debug for SourceRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SourceRef")
            .field("chunk_id", &self.chunk_id)
            .field("path", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SourceState {
    Current,
    Changed,
    Deleted,
    OutOfScope,
}
#[derive(Clone)]
pub struct RagSource {
    pub source: SourceRef,
    pub text: String,
}
#[derive(Clone)]
pub struct CheckedSource {
    pub source: SourceRef,
    pub state: SourceState,
    pub text: Option<String>,
}
pub trait SourceResolver: Send + Sync {
    /// Authorizes canonical paths against the current selected folders (including
    /// descendants/overlapping scopes), and tags the most specific content scope.
    fn retrieve(
        &self,
        question: &str,
        scope_ids: &[String],
        policy: &RagPolicy,
        top_k: usize,
        cancel: &AtomicBool,
    ) -> RagResult<Vec<RagSource>>;
    /// Current must mean the exact ID/SHA/path is current and authorized. The
    /// selected-folder check is resolver authority, not scope-ID equality.
    fn revalidate(
        &self,
        sources: &[SourceRef],
        scope_ids: &[String],
        policy: &RagPolicy,
        cancel: &AtomicBool,
    ) -> RagResult<Vec<CheckedSource>>;
}
pub type GenerationFuture<'a> =
    Pin<Box<dyn Future<Output = Result<GenerationResponse, GenerationError>> + Send + 'a>>;
pub trait RagTransport: Send + Sync {
    fn complete<'a>(
        &'a self,
        config: &'a GenerationConfig,
        messages: &'a [GenerationMessage],
        key: Option<&'a SecretValue>,
        cancel: &'a GenerationCancellation,
    ) -> GenerationFuture<'a>;
}
impl RagTransport for GenerationRuntime {
    fn complete<'a>(
        &'a self,
        config: &'a GenerationConfig,
        messages: &'a [GenerationMessage],
        key: Option<&'a SecretValue>,
        cancel: &'a GenerationCancellation,
    ) -> GenerationFuture<'a> {
        Box::pin(GenerationRuntime::complete(
            self, config, messages, key, cancel,
        ))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum QuestionMode {
    #[default]
    Documents,
    General,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RagRequest {
    pub request_id: String,
    pub conversation_id: String,
    pub question: String,
    #[serde(default)]
    pub scope_ids: Vec<String>,
    #[serde(default)]
    pub mode: QuestionMode,
    #[serde(default)]
    pub sources_hidden: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScopeSnapshot {
    pub revision: u64,
    pub selected_scope_ids: Vec<String>,
    pub scopes: Vec<RagScope>,
    pub external_questions_allowed: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TurnRole {
    User,
    Assistant,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TurnStatus {
    Pending,
    Complete,
    Cancelled,
    Failed,
    NoSources,
    Interrupted,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EvidenceStatus {
    NoSources,
    Unverified,
    SourceLinked,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RagCitation {
    pub label: String,
    pub source: SourceRef,
    pub state: SourceState,
    pub is_cited: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationTurn {
    pub id: String,
    pub conversation_id: String,
    pub request_id: String,
    pub sequence: u64,
    pub role: TurnRole,
    pub text: String,
    pub created_ms: u64,
    pub status: TurnStatus,
    pub scope_snapshot: ScopeSnapshot,
    pub sources_hidden: bool,
    pub citations: Vec<RagCitation>,
    pub provenance: Vec<SourceRef>,
    pub taint_complete: bool,
    pub evidence: EvidenceStatus,
    pub claims_verified: bool,
    pub unknown_citations: Vec<String>,
    pub provider: Option<ProviderSlot>,
    pub requested_model: Option<String>,
    pub reported_model: Option<String>,
    pub finish_reason: Option<String>,
    pub usage: Option<Value>,
    pub error: Option<RagError>,
}
impl fmt::Debug for ConversationTurn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConversationTurn")
            .field("id", &self.id)
            .field("role", &self.role)
            .field("status", &self.status)
            .field("text", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationSummary {
    pub id: String,
    pub title: String,
    pub created_ms: u64,
    pub updated_ms: u64,
    pub turn_count: u64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationPage {
    pub conversation: ConversationSummary,
    pub turns: Vec<ConversationTurn>,
    pub has_more: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderAttempt {
    pub provider: ProviderSlot,
    pub error: Option<RagError>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RagAnswer {
    pub conversation_id: String,
    pub request_id: String,
    pub user_turn: ConversationTurn,
    pub assistant_turn: ConversationTurn,
    pub attempts: Vec<ProviderAttempt>,
    pub omitted_history_pairs: usize,
}
impl fmt::Debug for RagAnswer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RagAnswer")
            .field("request_id", &self.request_id)
            .field("answer", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RagJob {
    pub request_id: String,
    pub conversation_id: String,
    pub phase: String,
    pub provider: Option<ProviderSlot>,
    pub error: Option<RagError>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialStatus {
    pub has_key: bool,
    pub support: SecretSupport,
}

fn slot_endpoint(
    slot: ProviderSlot,
    endpoint: &str,
) -> RagResult<crate::generation_runtime::EndpointInfo> {
    let info = validate_endpoint(endpoint).map_err(RagError::from)?;
    if info.is_local != (slot == ProviderSlot::Local) {
        return Err(RagError::new(RagErrorCode::Endpoint));
    }
    Ok(info)
}
