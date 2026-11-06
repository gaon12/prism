//! Read-only MCP (Model Context Protocol) client for external sources (requirement Q45).
//!
//! The user registers local MCP servers (a program plus arguments). Prism starts the program
//! itself — never through a shell — and talks JSON-RPC 2.0 over its stdin/stdout, one JSON
//! message per line (the MCP "stdio" transport).
//!
//! Prism only *reads* through MCP:
//! - A tool is usable only when the server marks it `readOnlyHint: true`, or the user allowed
//!   that exact tool name for that server. Everything else is listed but refused.
//! - Requests the server sends to Prism (sampling, elicitation, roots, ...) are declined.
//! - Every wait has a deadline, every message and result has a size cap, and the server process
//!   is killed when the client is dropped.
//!
//! Whether a question may be sent to an MCP server at all is decided by the caller with the same
//! external-sending permissions as RAG; this module only does the protocol safely.

// Used by the settings screen (connect/list_tools) and by answers (rag_external: search).
// A few protocol helpers are only exercised by tests, hence the allowance below.
#![cfg_attr(not(test), allow(dead_code))]

use serde_json::{json, Value};
use std::{
    collections::HashSet,
    io::{BufRead, BufReader, Read, Write},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError},
        Arc,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

/// MCP protocol revision Prism speaks. Servers answer with the version they chose.
pub const PROTOCOL_VERSION: &str = "2025-06-18";
/// Servers that may be enabled at once.
pub const MAX_SERVERS: usize = 8;
/// Longest single message accepted from a server.
const MAX_MESSAGE_BYTES: usize = 4 * 1024 * 1024;
/// Messages waiting to be read. The reader stops reading (so the server's writes block)
/// instead of queueing more, which bounds memory to about QUEUE × MAX_MESSAGE_BYTES.
const QUEUE: usize = 8;
/// Messages waiting to be written to a server that is slow to read its input.
const WRITE_QUEUE: usize = 4;
/// Text kept from one tool result (the rest is cut and marked).
const MAX_RESULT_CHARS: usize = 64 * 1024;
/// Tools kept from one server (a misbehaving server cannot flood the settings UI).
const MAX_TOOLS: usize = 256;
/// How often waits look at the cancel flag.
const TICK: Duration = Duration::from_millis(50);
/// How long Drop waits for the IO threads after killing the server.
const JOIN_WAIT: Duration = Duration::from_secs(2);

/// Which tool answers questions, and which of its arguments receives the question text.
/// Chosen by the user in settings and checked against the tool's input schema.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchMapping {
    pub tool: String,
    pub argument: String,
}

/// A server the user registered.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfig {
    /// Stable id (no credentials), used in saved provenance and for revoking a server.
    #[serde(default)]
    pub id: String,
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Tool names the user allowed even though the server did not mark them read-only.
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    /// The search tool for questions; None until the user picks one.
    #[serde(default)]
    pub search: Option<SearchMapping>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    /// Server says the tool does not change anything.
    pub read_only: bool,
    /// Prism will call it (read-only, or allowed by the user).
    pub usable: bool,
    /// The argument that can carry a question, when the schema makes that unambiguous.
    pub suggested_argument: Option<String>,
}

/// One tool call's text, with where it came from for the citation.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolText {
    pub server: String,
    pub tool: String,
    pub text: String,
    pub truncated: bool,
    /// The server flagged the result as an error (`isError`).
    pub is_error: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpError {
    /// The program could not be started.
    Start,
    /// No answer before the deadline.
    TimedOut,
    /// The caller cancelled.
    Cancelled,
    /// The server exited or closed its output.
    Closed,
    /// The server sent something that is not valid MCP.
    Protocol,
    /// The server answered with a JSON-RPC error (its message, shortened).
    Server(String),
    /// The tool is unknown or not read-only/allowed.
    NotAllowed,
    /// The chosen tool's input schema cannot be filled with just the question
    /// (the reason, for the settings screen).
    Schema(String),
}

/// Checks that `tool` can be called with only the question in `argument`: the schema must be an
/// object whose `argument` property is a string, and nothing else may be required.
pub fn check_mapping(tool: &Tool, argument: &str) -> Result<(), McpError> {
    let schema = &tool.input_schema;
    if schema.get("type").is_some_and(|kind| kind != "object") {
        return Err(McpError::Schema("input is not an object".into()));
    }
    let property = schema
        .pointer(&format!(
            "/properties/{}",
            argument.replace('~', "~0").replace('/', "~1")
        ))
        .ok_or_else(|| McpError::Schema(format!("no '{argument}' input")))?;
    if property.get("type").and_then(Value::as_str) != Some("string") {
        return Err(McpError::Schema(format!("'{argument}' is not text")));
    }
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|names| names.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if let Some(other) = required.iter().find(|name| **name != argument) {
        return Err(McpError::Schema(format!("'{other}' is also required")));
    }
    Ok(())
}

/// The single text input a question can go into, if the schema leaves no doubt.
fn suggest_argument(schema: &Value) -> Option<String> {
    let properties = schema.get("properties")?.as_object()?;
    let text: Vec<&String> = properties
        .iter()
        .filter(|(_, value)| value.get("type").and_then(Value::as_str) == Some("string"))
        .map(|(name, _)| name)
        .collect();
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|names| names.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    match (required.as_slice(), text.as_slice()) {
        ([only], _) if text.iter().any(|name| name.as_str() == *only) => Some(only.to_string()),
        ([], [only]) => Some(only.to_string()),
        _ => None,
    }
}

/// Where messages go and come from. The real one is a child process; tests use a fake server.
pub trait Transport {
    /// Queues one message, waiting at most until `deadline` for room.
    fn send(&mut self, line: &str, deadline: Instant) -> Result<(), McpError>;
    /// The next line from the server, waiting until `deadline`.
    fn recv(&mut self, deadline: Instant) -> Result<String, McpError>;
}

/// A started MCP server process and the two threads that talk to it.
///
/// Lifetime: on Windows the server is placed in a kill-on-close Job Object, so it and anything
/// it starts end when Prism ends for any reason. On Unix it gets its own process group, which
/// Drop kills as a whole; on Linux it also receives SIGKILL if Prism dies. On macOS a dying
/// Prism closes the server's input, which MCP servers treat as the signal to exit.
pub struct ChildTransport {
    child: Child,
    #[cfg(windows)]
    _job: crate::process_job::OwnedJobGuard,
    writer: Option<SyncSender<Vec<u8>>>,
    /// None once dropping: closing it releases a reader blocked on a full queue.
    lines: Option<Receiver<Result<String, McpError>>>,
    threads: Vec<JoinHandle<()>>,
}

impl ChildTransport {
    pub fn start(config: &ServerConfig) -> Result<Self, McpError> {
        let mut command = Command::new(&config.command);
        command
            .args(&config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Server logs are not shown or stored: they may contain user content.
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW: no console window flashes up for the server.
            command.creation_flags(0x0800_0000);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // Its own process group, so Drop can end the server and its children together.
            command.process_group(0);
            #[cfg(target_os = "linux")]
            // SAFETY: prctl is async-signal-safe and only changes this new process.
            unsafe {
                command.pre_exec(|| {
                    if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }
        #[cfg(windows)]
        let job = crate::process_job::OwnedJobGuard::new().map_err(|_| McpError::Start)?;
        let mut child = command.spawn().map_err(|_| McpError::Start)?;
        #[cfg(windows)]
        if job.assign(&child).is_err() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(McpError::Start);
        }
        let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(McpError::Start);
        };

        // Reader: stdout → bounded queue of lines.
        let (sender, lines) = mpsc::sync_channel(QUEUE);
        let reader = std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut buffer = Vec::new();
                // Read at most one byte past the cap, so a huge line cannot exhaust memory.
                let read = (&mut reader)
                    .take(MAX_MESSAGE_BYTES as u64 + 1)
                    .read_until(b'\n', &mut buffer);
                let message = match read {
                    Ok(0) | Err(_) => Err(McpError::Closed),
                    Ok(_) if buffer.len() > MAX_MESSAGE_BYTES => Err(McpError::Protocol),
                    Ok(_) => String::from_utf8(buffer).map_err(|_| McpError::Protocol),
                };
                let stop = message.is_err();
                // Blocks while the queue is full (back-pressure); fails once the client is gone.
                if sender.send(message).is_err() || stop {
                    break;
                }
            }
        });
        // Writer: bounded queue → stdin. A server that never reads only blocks this thread;
        // the client's send() gives up at its deadline and Drop then kills the server.
        let (writer, outgoing) = mpsc::sync_channel::<Vec<u8>>(WRITE_QUEUE);
        let writing = std::thread::spawn(move || {
            for message in outgoing {
                if stdin
                    .write_all(&message)
                    .and_then(|_| stdin.flush())
                    .is_err()
                {
                    break;
                }
            }
            // Dropping stdin here closes the server's input.
        });
        Ok(Self {
            child,
            #[cfg(windows)]
            _job: job,
            writer: Some(writer),
            lines: Some(lines),
            threads: vec![reader, writing],
        })
    }
}

impl Transport for ChildTransport {
    fn send(&mut self, line: &str, deadline: Instant) -> Result<(), McpError> {
        let writer = self.writer.as_ref().ok_or(McpError::Closed)?;
        let mut message = Vec::with_capacity(line.len() + 1);
        message.extend_from_slice(line.as_bytes());
        message.push(b'\n');
        loop {
            match writer.try_send(message) {
                Ok(()) => return Ok(()),
                Err(TrySendError::Disconnected(_)) => return Err(McpError::Closed),
                Err(TrySendError::Full(back)) => {
                    if Instant::now() >= deadline {
                        return Err(McpError::TimedOut);
                    }
                    message = back;
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        }
    }
    fn recv(&mut self, deadline: Instant) -> Result<String, McpError> {
        let wait = deadline.saturating_duration_since(Instant::now());
        let lines = self.lines.as_ref().ok_or(McpError::Closed)?;
        match lines.recv_timeout(wait) {
            Ok(line) => line,
            Err(RecvTimeoutError::Timeout) => Err(McpError::TimedOut),
            Err(RecvTimeoutError::Disconnected) => Err(McpError::Closed),
        }
    }
}

impl Drop for ChildTransport {
    fn drop(&mut self) {
        // Stop feeding the writer and stop accepting lines (a reader blocked on a full queue
        // wakes up with an error), then end the server and everything it started.
        self.writer.take();
        self.lines.take();
        #[cfg(unix)]
        // SAFETY: process_group(0) made the server's pid its group id, and the pid stays
        // reserved until the wait() below, so this signal reaches only that group.
        unsafe {
            libc::kill(-(self.child.id() as libc::pid_t), libc::SIGKILL);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        // Killing closes the pipes, so both threads finish; wait briefly for them. A process
        // that escaped the group/job could hold a pipe open, so never wait forever.
        let until = Instant::now() + JOIN_WAIT;
        for thread in self.threads.drain(..) {
            while !thread.is_finished() && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(10));
            }
            if thread.is_finished() {
                let _ = thread.join();
            }
        }
    }
}

/// A connected, initialized server.
pub struct Client<T: Transport> {
    transport: T,
    server: String,
    allowed: HashSet<String>,
    next_id: u64,
    timeout: Duration,
    tools: Vec<Tool>,
    /// Set by the caller to stop waiting; checked at least every `TICK`.
    cancel: Arc<AtomicBool>,
}

impl Client<ChildTransport> {
    /// Starts the server program and performs the MCP handshake.
    pub fn connect(
        config: &ServerConfig,
        timeout: Duration,
        cancel: Arc<AtomicBool>,
    ) -> Result<Self, McpError> {
        if cancel.load(Ordering::Acquire) {
            return Err(McpError::Cancelled);
        }
        Client::initialize(ChildTransport::start(config)?, config, timeout, cancel)
    }
}

impl<T: Transport> Client<T> {
    /// MCP handshake: `initialize` request, then the `notifications/initialized` notice.
    pub fn initialize(
        transport: T,
        config: &ServerConfig,
        timeout: Duration,
        cancel: Arc<AtomicBool>,
    ) -> Result<Self, McpError> {
        let mut client = Self {
            cancel,
            transport,
            server: config.name.clone(),
            allowed: config.allowed_tools.iter().cloned().collect(),
            next_id: 1,
            timeout,
            tools: Vec::new(),
        };
        let result = client.request(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                // Prism offers no client features (no sampling, roots or elicitation).
                "capabilities": {},
                "clientInfo": { "name": "Prism", "version": env!("CARGO_PKG_VERSION") },
            }),
        )?;
        if !result.get("capabilities").is_some_and(Value::is_object) {
            return Err(McpError::Protocol);
        }
        client.notify("notifications/initialized")?;
        Ok(client)
    }

    /// Lists the server's tools (following pagination) and remembers them for `call`.
    pub fn list_tools(&mut self) -> Result<Vec<Tool>, McpError> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let params = match &cursor {
                Some(cursor) => json!({ "cursor": cursor }),
                None => json!({}),
            };
            let result = self.request("tools/list", params)?;
            let page = result
                .get("tools")
                .and_then(Value::as_array)
                .ok_or(McpError::Protocol)?;
            for item in page {
                if tools.len() >= MAX_TOOLS {
                    break;
                }
                if let Some(tool) = self.parse_tool(item) {
                    tools.push(tool);
                }
            }
            cursor = result
                .get("nextCursor")
                .and_then(Value::as_str)
                .map(str::to_string);
            if cursor.is_none() || tools.len() >= MAX_TOOLS {
                break;
            }
        }
        self.tools = tools.clone();
        Ok(tools)
    }

    fn parse_tool(&self, item: &Value) -> Option<Tool> {
        let name = item.get("name")?.as_str()?.to_string();
        let read_only = item
            .pointer("/annotations/readOnlyHint")
            .and_then(Value::as_bool)
            == Some(true);
        let input_schema = item.get("inputSchema").cloned().unwrap_or(json!({}));
        Some(Tool {
            usable: read_only || self.allowed.contains(&name),
            suggested_argument: suggest_argument(&input_schema),
            description: item
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .chars()
                .take(1000)
                .collect(),
            input_schema,
            read_only,
            name,
        })
    }

    /// Calls a usable tool and returns its text content. Call `list_tools` first.
    pub fn call(&mut self, tool: &str, arguments: Value) -> Result<ToolText, McpError> {
        if !self
            .tools
            .iter()
            .any(|known| known.name == tool && known.usable)
        {
            return Err(McpError::NotAllowed);
        }
        let result = self.request(
            "tools/call",
            json!({ "name": tool, "arguments": arguments }),
        )?;
        let (text, truncated) = result_text(&result);
        Ok(ToolText {
            server: self.server.clone(),
            tool: tool.to_string(),
            text,
            truncated,
            is_error: result.get("isError").and_then(Value::as_bool) == Some(true),
        })
    }

    /// Asks the user-chosen search tool with the question as its only argument. The mapping
    /// is checked against the tool's current schema first; a schema that needs more than the
    /// question is reported as `Schema`, never guessed.
    pub fn search(
        &mut self,
        mapping: &SearchMapping,
        question: &str,
    ) -> Result<ToolText, McpError> {
        let tool = self
            .tools
            .iter()
            .find(|tool| tool.name == mapping.tool && tool.usable)
            .ok_or(McpError::NotAllowed)?;
        check_mapping(tool, &mapping.argument)?;
        let mut arguments = serde_json::Map::new();
        arguments.insert(
            mapping.argument.clone(),
            Value::String(question.to_string()),
        );
        self.call(&mapping.tool, Value::Object(arguments))
    }

    fn notify(&mut self, method: &str) -> Result<(), McpError> {
        let message = json!({ "jsonrpc": "2.0", "method": method });
        let deadline = Instant::now() + self.timeout;
        self.transport.send(&message.to_string(), deadline)
    }

    fn cancelled(&self) -> Result<(), McpError> {
        if self.cancel.load(Ordering::Acquire) {
            Err(McpError::Cancelled)
        } else {
            Ok(())
        }
    }

    /// Sends a request and waits for the response with the same id. Notifications from the
    /// server are skipped; requests from the server are declined.
    fn request(&mut self, method: &str, params: Value) -> Result<Value, McpError> {
        let id = self.next_id;
        self.next_id += 1;
        let message = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let deadline = Instant::now() + self.timeout;
        self.cancelled()?;
        self.transport.send(&message.to_string(), deadline)?;
        loop {
            self.cancelled()?;
            // Wait in short slices so a cancel is noticed quickly.
            let line = match self.transport.recv(deadline.min(Instant::now() + TICK)) {
                Err(McpError::TimedOut) if Instant::now() < deadline => continue,
                other => other?,
            };
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let incoming: Value = serde_json::from_str(line).map_err(|_| McpError::Protocol)?;
            if incoming.get("method").is_some() {
                // A server request has an id and expects an answer; a notification has none.
                if let Some(request_id) = incoming.get("id") {
                    let decline = json!({
                        "jsonrpc": "2.0",
                        "id": request_id,
                        "error": { "code": -32601, "message": "Prism does not offer this." },
                    });
                    self.transport.send(&decline.to_string(), deadline)?;
                }
                continue;
            }
            if incoming.get("id").and_then(Value::as_u64) != Some(id) {
                // A late answer to an earlier, timed-out request.
                continue;
            }
            if let Some(error) = incoming.get("error") {
                let text = error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("error");
                return Err(McpError::Server(text.chars().take(200).collect()));
            }
            return incoming.get("result").cloned().ok_or(McpError::Protocol);
        }
    }
}

/// Joins the text parts of a tool result. Other content (images, audio, binary resources) is
/// named but not included; structured-only results are shown as JSON.
fn result_text(result: &Value) -> (String, bool) {
    let mut parts = Vec::new();
    for item in result
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        match item.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = item.get("text").and_then(Value::as_str) {
                    parts.push(text.to_string());
                }
            }
            Some("resource") => {
                if let Some(text) = item.pointer("/resource/text").and_then(Value::as_str) {
                    parts.push(text.to_string());
                }
            }
            Some("resource_link") => {
                if let Some(uri) = item.get("uri").and_then(Value::as_str) {
                    parts.push(format!("[{uri}]"));
                }
            }
            Some(other) => parts.push(format!("[{other} omitted]")),
            None => {}
        }
    }
    if parts.is_empty() {
        if let Some(structured) = result.get("structuredContent") {
            parts.push(structured.to_string());
        }
    }
    let joined = parts.join("\n\n");
    if joined.chars().count() > MAX_RESULT_CHARS {
        (joined.chars().take(MAX_RESULT_CHARS).collect(), true)
    } else {
        (joined, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// A scripted server: answers each request by method name, and records what Prism sent.
    struct Fake {
        sent: Vec<Value>,
        queue: VecDeque<String>,
        answer: fn(&Value) -> Vec<Value>,
    }
    impl Transport for Fake {
        fn send(&mut self, line: &str, _: Instant) -> Result<(), McpError> {
            let message: Value = serde_json::from_str(line).unwrap();
            // Prism's answers to the server's own requests have no method: nothing to reply to.
            let replies = if message.get("method").is_some() {
                (self.answer)(&message)
            } else {
                vec![]
            };
            for reply in replies {
                self.queue.push_back(reply.to_string());
            }
            self.sent.push(message);
            Ok(())
        }
        fn recv(&mut self, _: Instant) -> Result<String, McpError> {
            self.queue.pop_front().ok_or(McpError::TimedOut)
        }
    }
    fn reply(message: &Value, result: Value) -> Value {
        json!({ "jsonrpc": "2.0", "id": message["id"], "result": result })
    }
    fn server(message: &Value) -> Vec<Value> {
        match message["method"].as_str().unwrap() {
            "initialize" => vec![reply(
                message,
                json!({ "protocolVersion": PROTOCOL_VERSION, "capabilities": { "tools": {} } }),
            )],
            "tools/list" if message["params"].get("cursor").is_none() => vec![
                // Noise first: a log notification and a sampling request Prism must decline.
                json!({ "jsonrpc": "2.0", "method": "notifications/message", "params": {} }),
                json!({ "jsonrpc": "2.0", "id": "s1", "method": "sampling/createMessage", "params": {} }),
                reply(
                    message,
                    json!({
                        "tools": [
                            { "name": "search", "description": "Find pages", "inputSchema": { "type": "object", "properties": { "searchTerm": { "type": "string" }, "limit": { "type": "integer" } }, "required": ["searchTerm"] }, "annotations": { "readOnlyHint": true } },
                            { "name": "delete_page", "annotations": { "readOnlyHint": false } },
                        ],
                        "nextCursor": "2",
                    }),
                ),
            ],
            "tools/list" => vec![reply(message, json!({ "tools": [ { "name": "fetch" } ] }))],
            "tools/call" => vec![reply(
                message,
                json!({
                    "content": [
                        { "type": "text", "text": "Budget rose 18%." },
                        { "type": "image", "data": "AAAA", "mimeType": "image/png" },
                        { "type": "resource", "resource": { "uri": "x://1", "text": "Appendix" } },
                    ],
                }),
            )],
            _ => vec![],
        }
    }
    fn config(allowed: &[&str]) -> ServerConfig {
        ServerConfig {
            id: "wiki-1".into(),
            search: None,
            name: "wiki".into(),
            command: "unused".into(),
            args: vec![],
            allowed_tools: allowed.iter().map(|s| s.to_string()).collect(),
        }
    }
    fn connect(allowed: &[&str]) -> Client<Fake> {
        let fake = Fake {
            sent: vec![],
            queue: VecDeque::new(),
            answer: server,
        };
        Client::initialize(
            fake,
            &config(allowed),
            Duration::from_secs(1),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap()
    }

    #[test]
    fn handshake_lists_pages_and_declines_server_requests() {
        let mut client = connect(&["fetch"]);
        let tools = client.list_tools().unwrap();
        let summary: Vec<_> = tools
            .iter()
            .map(|t| (t.name.as_str(), t.read_only, t.usable))
            .collect();
        assert_eq!(
            summary,
            [
                ("search", true, true),
                ("delete_page", false, false),
                ("fetch", false, true)
            ]
        );

        let sent = &client.transport.sent;
        assert_eq!(sent[0]["method"], "initialize");
        assert_eq!(sent[0]["params"]["capabilities"], json!({}));
        assert_eq!(sent[1]["method"], "notifications/initialized");
        assert!(sent[1].get("id").is_none(), "a notification has no id");
        // The sampling request was answered with an error, not served.
        let decline = sent.iter().find(|m| m["id"] == "s1").unwrap();
        assert_eq!(decline["error"]["code"], -32601);
        assert_eq!(sent.last().unwrap()["params"]["cursor"], "2");
    }

    #[test]
    fn only_read_only_or_allowed_tools_are_called() {
        let mut client = connect(&[]);
        client.list_tools().unwrap();
        assert_eq!(
            client.call("delete_page", json!({})).unwrap_err(),
            McpError::NotAllowed
        );
        assert_eq!(
            client.call("unknown", json!({})).unwrap_err(),
            McpError::NotAllowed
        );
        let text = client.call("search", json!({ "query": "budget" })).unwrap();
        assert_eq!(text.text, "Budget rose 18%.\n\n[image omitted]\n\nAppendix");
        assert_eq!(
            (text.server.as_str(), text.tool.as_str()),
            ("wiki", "search")
        );
        assert!(!text.truncated && !text.is_error);
    }

    #[test]
    fn errors_timeouts_and_long_results() {
        fn failing(message: &Value) -> Vec<Value> {
            match message["method"].as_str().unwrap() {
                "initialize" => vec![
                    json!({ "jsonrpc": "2.0", "id": message["id"], "error": { "code": -1, "message": "no" } }),
                ],
                _ => vec![],
            }
        }
        let fake = Fake {
            sent: vec![],
            queue: VecDeque::new(),
            answer: failing,
        };
        let error = Client::initialize(
            fake,
            &config(&[]),
            Duration::from_secs(1),
            Arc::new(AtomicBool::new(false)),
        )
        .err()
        .unwrap();
        assert_eq!(error, McpError::Server("no".into()));

        fn silent(_: &Value) -> Vec<Value> {
            vec![]
        }
        let fake = Fake {
            sent: vec![],
            queue: VecDeque::new(),
            answer: silent,
        };
        let error = Client::initialize(
            fake,
            &config(&[]),
            Duration::from_secs(1),
            Arc::new(AtomicBool::new(false)),
        )
        .err()
        .unwrap();
        assert_eq!(error, McpError::TimedOut);

        let long = "가".repeat(MAX_RESULT_CHARS + 5);
        let (text, truncated) =
            result_text(&json!({ "content": [{ "type": "text", "text": long }] }));
        assert!(truncated);
        assert_eq!(text.chars().count(), MAX_RESULT_CHARS);
        let (text, _) = result_text(&json!({ "structuredContent": { "n": 1 } }));
        assert_eq!(text, "{\"n\":1}");
    }

    #[test]
    fn search_uses_only_an_explicit_mapping_checked_against_the_schema() {
        let mut client = connect(&[]);
        let tools = client.list_tools().unwrap();
        assert_eq!(tools[0].suggested_argument.as_deref(), Some("searchTerm"));

        let good = SearchMapping {
            tool: "search".into(),
            argument: "searchTerm".into(),
        };
        let text = client.search(&good, "budget 2025").unwrap();
        assert_eq!(text.tool, "search");
        let call = client
            .transport
            .sent
            .iter()
            .rev()
            .find(|m| m["method"] == "tools/call")
            .unwrap();
        assert_eq!(
            call["params"]["arguments"],
            json!({ "searchTerm": "budget 2025" })
        );

        // A guessed "query" argument is refused with the reason, not sent.
        let guessed = SearchMapping {
            tool: "search".into(),
            argument: "query".into(),
        };
        assert_eq!(
            client.search(&guessed, "x").unwrap_err(),
            McpError::Schema("no 'query' input".into())
        );
        let wrong_type = SearchMapping {
            tool: "search".into(),
            argument: "limit".into(),
        };
        assert_eq!(
            client.search(&wrong_type, "x").unwrap_err(),
            McpError::Schema("'limit' is not text".into())
        );
        let not_read_only = SearchMapping {
            tool: "delete_page".into(),
            argument: "q".into(),
        };
        assert_eq!(
            client.search(&not_read_only, "x").unwrap_err(),
            McpError::NotAllowed
        );
    }

    #[test]
    fn schemas_needing_more_than_the_question_are_reported() {
        let tool = |schema: Value| Tool {
            name: "t".into(),
            description: String::new(),
            suggested_argument: suggest_argument(&schema),
            input_schema: schema,
            read_only: true,
            usable: true,
        };
        let two = tool(json!({ "type": "object",
            "properties": { "q": { "type": "string" }, "site": { "type": "string" } },
            "required": ["q", "site"] }));
        assert_eq!(
            check_mapping(&two, "q"),
            Err(McpError::Schema("'site' is also required".into()))
        );
        assert_eq!(two.suggested_argument, None, "ambiguous: nothing suggested");
        let single = tool(json!({ "properties": { "text": { "type": "string" } } }));
        assert_eq!(single.suggested_argument.as_deref(), Some("text"));
        assert!(check_mapping(&single, "text").is_ok());
        let array = tool(json!({ "type": "array" }));
        assert!(matches!(
            check_mapping(&array, "q"),
            Err(McpError::Schema(_))
        ));
    }

    #[test]
    fn cancel_stops_waiting_quickly() {
        fn silent(_: &Value) -> Vec<Value> {
            vec![]
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&cancel);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            flag.store(true, Ordering::Release);
        });
        let fake = Fake {
            sent: vec![],
            queue: VecDeque::new(),
            answer: silent,
        };
        let started = Instant::now();
        let error = Client::initialize(fake, &config(&[]), Duration::from_secs(30), cancel)
            .err()
            .unwrap();
        assert_eq!(error, McpError::Cancelled);
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
