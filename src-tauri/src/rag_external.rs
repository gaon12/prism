//! External evidence from MCP servers in chat answers (requirement Q45, step 2).
//!
//! How it fits into RAG, in plain words:
//!
//! - Each search result becomes an ordinary [`SourceRef`] with a special scope id
//!   (`mcp_<server id>_<fingerprint prefix>`) and the server's *descriptor fingerprint* in the
//!   `sha256` field. Because it is a normal source, the existing provenance machinery carries
//!   it into later turns, history and document drafts exactly like a local document excerpt.
//! - The fingerprint covers everything that decides which program runs and what it is asked
//!   (id, program, arguments, allowed tools, search tool and input). If any of that changes, or
//!   the server is removed, or external sources are switched off, old results no longer match
//!   the current settings and count as revoked: their text and the answers built on them are
//!   hidden, and they are never sent to a model again.
//! - Only the question text is sent to a server. Server output is untrusted quoted material:
//!   it is labelled `[E1]`, `[E2]`… separately from local document labels `[S1]`…, and the
//!   model is told never to follow instructions inside it.
//! - No program path, argument, environment or credential is stored with a turn: only the
//!   server id and name the user chose, the tool name, the fingerprint hash and the result.
use std::{
    fmt,
    sync::{atomic::AtomicBool, Arc},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    mcp_client::{Client, McpError, ServerConfig},
    rag_service::SourceRef,
};

/// Scope ids of external sources start with this. Library folder ids never do.
pub const SCOPE_PREFIX: &str = "mcp_";
/// Time allowed for one server (start, handshake and search).
pub const PER_SERVER: Duration = Duration::from_secs(10);
/// Time allowed for all servers of one question together.
pub const TOTAL: Duration = Duration::from_secs(20);
/// Characters kept from one server's result, and from all results of one question.
pub const MAX_CHARS_PER_SERVER: usize = 8_000;
pub const MAX_CHARS_TOTAL: usize = 24_000;

/// SHA-256 (hex) of everything that decides what runs and what it is asked.
pub fn fingerprint(server: &ServerConfig) -> String {
    let descriptor = serde_json::json!({
        "id": server.id,
        "command": server.command,
        "args": server.args,
        "allowedTools": server.allowed_tools,
        "search": server.search,
    });
    format!("{:x}", Sha256::digest(descriptor.to_string().as_bytes()))
}

/// The scope id saved with this server's results.
pub fn scope_id(server: &ServerConfig) -> String {
    format!("{SCOPE_PREFIX}{}_{}", server.id, &fingerprint(server)[..24])
}

pub fn is_external(source: &SourceRef) -> bool {
    source.scope_id.starts_with(SCOPE_PREFIX)
}

/// True while the server that produced `source` is still configured exactly as it was.
/// `servers` is empty when external sources are switched off, so everything is revoked then.
pub fn allowed(servers: &[ServerConfig], source: &SourceRef) -> bool {
    servers.iter().any(|server| {
        server.search.is_some()
            && scope_id(server) == source.scope_id
            && fingerprint(server) == source.sha256
    })
}

/// What happened when one server was asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExternalOutcome {
    /// Returned text, used as evidence.
    Ok,
    /// Answered, but with nothing usable (empty, or the server flagged an error).
    Empty,
    TimedOut,
    Cancelled,
    /// The tool's input no longer matches the saved mapping (see settings).
    Schema,
    /// The program could not start.
    Start,
    /// Protocol/server failure.
    Failed,
    /// The server's settings changed or it was removed while the question ran.
    Changed,
    /// Not asked because the time for all servers ran out.
    Skipped,
}

/// One server's contribution to an answer, saved with the assistant turn.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalEvidence {
    /// `E1`, `E2`… when the result was given to the model, otherwise `None`.
    pub label: Option<String>,
    pub server_id: String,
    pub server_name: String,
    pub tool: String,
    /// Descriptor fingerprint of the server when it was asked (a hash, no program details).
    #[serde(default)]
    pub fingerprint: String,
    pub outcome: ExternalOutcome,
    /// The (shortened) result. Cleared when the source is revoked.
    pub text: String,
    #[serde(default)]
    pub truncated: bool,
    /// Set when shown after the server was removed/changed or external sources were turned off.
    #[serde(default)]
    pub revoked: bool,
}

/// A saved excerpt bound to the exact reference used by an immutable document.
/// Program paths, process arguments and credentials never enter this DTO.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentExternalSource {
    /// None for a server outcome which supplied no bound document evidence.
    pub source: Option<SourceRef>,
    pub evidence: ExternalEvidence,
}
impl fmt::Debug for ExternalEvidence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExternalEvidence")
            .field("label", &self.label)
            .field("server_id", &self.server_id)
            .field("outcome", &self.outcome)
            .field("text", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

/// The text a server returned.
pub struct SearchText {
    pub text: String,
    pub truncated: bool,
    pub is_error: bool,
}

/// Runs one search on one server. Replaceable in tests (no real processes).
pub trait ExternalSearch: Send + Sync {
    fn search(
        &self,
        server: &ServerConfig,
        question: &str,
        timeout: Duration,
        cancel: Arc<AtomicBool>,
    ) -> Result<SearchText, McpError>;
}

/// The real implementation: start the server, run its mapped search tool, stop it again.
pub struct McpSearch;
impl ExternalSearch for McpSearch {
    fn search(
        &self,
        server: &ServerConfig,
        question: &str,
        timeout: Duration,
        cancel: Arc<AtomicBool>,
    ) -> Result<SearchText, McpError> {
        let mapping = server.search.as_ref().ok_or(McpError::NotAllowed)?;
        let mut client = Client::connect(server, timeout, cancel)?;
        // A fresh chat request owns a fresh connection. Discover its current
        // tools before validating the saved mapping and read-only permission;
        // a previous settings probe belongs to a different, closed connection.
        client.list_tools()?;
        let result = client.search(mapping, question)?;
        Ok(SearchText {
            text: result.text,
            truncated: result.truncated,
            is_error: result.is_error,
        })
        // Dropping `client` stops the server and its child processes.
    }
}

pub fn outcome(error: &McpError) -> ExternalOutcome {
    match error {
        McpError::TimedOut => ExternalOutcome::TimedOut,
        McpError::Cancelled => ExternalOutcome::Cancelled,
        McpError::Schema(_) | McpError::NotAllowed => ExternalOutcome::Schema,
        McpError::Start => ExternalOutcome::Start,
        McpError::Closed | McpError::Protocol | McpError::Server(_) => ExternalOutcome::Failed,
    }
}

/// The saved source for one result: the descriptor fingerprint stands in for a content hash.
pub fn source_ref(server: &ServerConfig, request_id: &str, label: &str) -> SourceRef {
    let tool = server
        .search
        .as_ref()
        .map(|mapping| mapping.tool.as_str())
        .unwrap_or_default();
    SourceRef {
        chunk_id: format!("{label}-{request_id}"),
        scope_id: scope_id(server),
        file_path: format!("mcp://{}/{}", server.id, tool),
        sha256: fingerprint(server),
        page: None,
        paragraph: None,
    }
}

/// Keeps at most `max` characters and says whether anything was cut.
/// True while the server that produced `evidence` is still configured exactly as it was.
pub fn evidence_allowed(servers: &[ServerConfig], evidence: &ExternalEvidence) -> bool {
    servers
        .iter()
        .any(|server| server.search.is_some() && fingerprint(server) == evidence.fingerprint)
}

pub fn shorten(text: &str, max: usize) -> (String, bool) {
    let mut chars = text.trim().chars();
    let kept: String = chars.by_ref().take(max).collect();
    let cut = chars.next().is_some();
    (kept, cut)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp_client::SearchMapping;

    fn server() -> ServerConfig {
        ServerConfig {
            id: "wiki".into(),
            name: "Wiki".into(),
            command: "node".into(),
            args: vec!["server.js".into()],
            allowed_tools: vec![],
            search: Some(SearchMapping {
                tool: "search".into(),
                argument: "query".into(),
            }),
        }
    }

    #[test]
    fn any_descriptor_change_revokes_old_results() {
        let original = server();
        let source = source_ref(&original, "request1", "E1");
        assert!(is_external(&source));
        assert!(allowed(std::slice::from_ref(&original), &source));
        // Turning external sources off empties the list.
        assert!(!allowed(&[], &source));
        // Same id, different program / arguments / mapping / allowed tools: revoked.
        let mut changed = original.clone();
        changed.command = "other".into();
        assert!(!allowed(&[changed], &source));
        let mut changed = original.clone();
        changed.args.push("--more".into());
        assert!(!allowed(&[changed], &source));
        let mut changed = original.clone();
        changed.search.as_mut().unwrap().argument = "q".into();
        assert!(!allowed(&[changed], &source));
        let mut changed = original.clone();
        changed.allowed_tools.push("write".into());
        assert!(!allowed(&[changed], &source));
        // A mapping removed from the same server: revoked.
        let mut changed = original;
        changed.search = None;
        assert!(!allowed(&[changed], &source));
    }

    #[test]
    fn saved_source_holds_no_program_details() {
        let source = source_ref(&server(), "request1", "E2");
        let saved = serde_json::to_string(&source).unwrap();
        assert!(!saved.contains("node") && !saved.contains("server.js"));
        assert_eq!(source.file_path, "mcp://wiki/search");
        assert_eq!(source.sha256.len(), 64);
        assert!(source.scope_id.len() <= 128);
    }

    #[test]
    fn shorten_counts_characters_not_bytes() {
        assert_eq!(shorten("  가나다라  ", 2), ("가나".into(), true));
        assert_eq!(shorten("abc", 3), ("abc".into(), false));
    }

    #[test]
    fn real_adapter_discovers_tools_before_sending_only_the_question() {
        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("mcp-fixture.mjs");
        let trace = directory.path().join("trace.jsonl");
        let control = directory.path().join("control.json");
        std::fs::write(
            &script,
            include_str!("../../tests/native/fixtures/mcp-server.mjs"),
        )
        .unwrap();
        std::fs::write(
            &control,
            r#"{"mode":"normal","text":"실제 stdio 검색 결과"}"#,
        )
        .unwrap();
        let mut config = server();
        config.args = [&script, &trace, &control]
            .map(|path| path.to_string_lossy().into_owned())
            .to_vec();
        let found = McpSearch
            .search(
                &config,
                "보낼 질문만",
                Duration::from_secs(5),
                Arc::new(AtomicBool::new(false)),
            )
            .expect("the actual adapter must discover and call the fixture's read-only tool");
        assert_eq!(found.text, "실제 stdio 검색 결과");
        let events: Vec<serde_json::Value> = std::fs::read_to_string(trace)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let methods: Vec<_> = events
            .iter()
            .filter_map(|event| event.pointer("/message/method").and_then(|v| v.as_str()))
            .collect();
        assert_eq!(
            methods,
            [
                "initialize",
                "notifications/initialized",
                "tools/list",
                "tools/call"
            ]
        );
        let call = events
            .iter()
            .find_map(|event| {
                let message = event.get("message")?;
                (message["method"] == "tools/call").then_some(message)
            })
            .unwrap();
        assert_eq!(
            call["params"]["arguments"],
            serde_json::json!({"query": "보낼 질문만"})
        );
        // A discovered tool is still refused when it is not read-only/allowed.
        config.search.as_mut().unwrap().tool = "delete".into();
        assert_eq!(
            McpSearch
                .search(
                    &config,
                    "보낼 질문만",
                    Duration::from_secs(5),
                    Arc::new(AtomicBool::new(false))
                )
                .err(),
            Some(McpError::NotAllowed)
        );
    }
}
