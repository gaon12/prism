//! Non-streaming OpenAI-compatible Chat Completions transport.
//! This is deliberately not a frontend IPC surface. The RAG caller must recheck
//! current content-root/external-send permissions before every invocation.
use crate::secret_store::SecretValue;
use reqwest::{
    header::{HeaderValue, AUTHORIZATION, CONTENT_TYPE},
    Client, StatusCode, Url,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fmt,
    future::Future,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use zeroize::Zeroizing;

const MAX_REQUEST_BYTES: usize = 1024 * 1024;
const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const MAX_SCHEMA_BYTES: usize = 64 * 1024;
const CANCEL_POLL: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GenerationErrorCode {
    Input,
    Endpoint,
    Network,
    Auth,
    RateLimited,
    Context,
    Server,
    Malformed,
    Cancelled,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationError {
    pub code: GenerationErrorCode,
    pub message: String,
}
impl GenerationError {
    fn new(code: GenerationErrorCode) -> Self {
        let message = match code {
            GenerationErrorCode::Input => "Check the model, messages, token limit, and sampling settings.",
            GenerationErrorCode::Endpoint => "Use a valid OpenAI-compatible endpoint. HTTP is allowed only for exact loopback hosts; remote endpoints require HTTPS without credentials, query, or fragment.",
            GenerationErrorCode::Network => "The generation server could not be reached or its response timed out.",
            GenerationErrorCode::Auth => "An API key is required or the generation server rejected authentication.",
            GenerationErrorCode::RateLimited => "The generation server is rate limiting requests. Try again later.",
            GenerationErrorCode::Context => "The generation request exceeds the server's context or input limit. Reduce the selected context or token limit.",
            GenerationErrorCode::Server => "The generation server could not complete the request.",
            GenerationErrorCode::Malformed => "The generation server returned an invalid, unsupported, or oversized response.",
            GenerationErrorCode::Cancelled => "Generation cancelled.",
        };
        Self {
            code,
            message: message.into(),
        }
    }
}
impl fmt::Display for GenerationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}
impl std::error::Error for GenerationError {}
fn error(code: GenerationErrorCode) -> GenerationError {
    GenerationError::new(code)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TokenParameter {
    #[default]
    MaxTokens,
    MaxCompletionTokens,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationConfig {
    pub endpoint: String,
    pub model: String,
    pub max_tokens: u32,
    #[serde(default)]
    pub token_parameter: TokenParameter,
    pub temperature: Option<f32>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GenerationRole {
    System,
    User,
    Assistant,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct GenerationMessage {
    pub role: GenerationRole,
    pub content: String,
}
impl fmt::Debug for GenerationMessage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GenerationMessage")
            .field("role", &self.role)
            .field("content", &"[REDACTED]")
            .finish()
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationResponse {
    pub text: String,
    pub model: Option<String>,
    pub finish_reason: Option<String>,
    /// Preserve provider-reported usage, including provider extensions. Never estimate it.
    pub usage: Option<Value>,
}
impl fmt::Debug for GenerationResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GenerationResponse")
            .field("text", &"[REDACTED]")
            .field("has_finish_reason", &self.finish_reason.is_some())
            .field("has_usage", &self.usage.is_some())
            .finish()
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EndpointInfo {
    pub base_url: String,
    pub completion_url: String,
    pub is_local: bool,
}
impl fmt::Debug for EndpointInfo {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EndpointInfo")
            .field("is_local", &self.is_local)
            .finish_non_exhaustive()
    }
}

/// Classification is based on the configured host, never on a caller-saved isLocal flag.
/// Alternate numeric loopback spellings and localhost suffixes are not local.
pub fn validate_endpoint(input: &str) -> Result<EndpointInfo, GenerationError> {
    if input.len() > 2048 || input.chars().any(char::is_control) || input.contains('\\') {
        return Err(error(GenerationErrorCode::Endpoint));
    }
    let input = input.trim();
    let mut url = Url::parse(input).map_err(|_| error(GenerationErrorCode::Endpoint))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.port() == Some(0)
    {
        return Err(error(GenerationErrorCode::Endpoint));
    }
    let authority = input
        .split_once("://")
        .map(|(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or(""))
        .ok_or_else(|| error(GenerationErrorCode::Endpoint))?;
    if authority.contains('@') {
        return Err(error(GenerationErrorCode::Endpoint));
    }
    let host = if authority.starts_with('[') {
        authority
            .find(']')
            .map(|end| &authority[..=end])
            .unwrap_or("")
    } else {
        authority.split(':').next().unwrap_or("")
    };
    let is_local = matches!(
        host.to_ascii_lowercase().as_str(),
        "localhost" | "127.0.0.1" | "[::1]"
    ) && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if url.scheme() == "http" && !is_local {
        return Err(error(GenerationErrorCode::Endpoint));
    }
    let path = url.path().trim_end_matches('/').to_owned();
    let base_path = if let Some(base) = path.strip_suffix("/chat/completions") {
        base
    } else if path.is_empty() {
        "/v1"
    } else {
        &path
    };
    url.set_path(base_path);
    let base_url = url.to_string().trim_end_matches('/').to_owned();
    let mut completion = url;
    completion
        .path_segments_mut()
        .map_err(|_| error(GenerationErrorCode::Endpoint))?
        .pop_if_empty()
        .push("chat")
        .push("completions");
    Ok(EndpointInfo {
        base_url,
        completion_url: completion.into(),
        is_local,
    })
}

#[derive(Clone, Default)]
pub struct GenerationCancellation {
    cancelled: Arc<AtomicBool>,
}
impl GenerationCancellation {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
    fn check(&self) -> Result<(), GenerationError> {
        if self.is_cancelled() {
            Err(error(GenerationErrorCode::Cancelled))
        } else {
            Ok(())
        }
    }
}

#[derive(Clone)]
pub struct GenerationRuntime {
    connect_timeout: Duration,
    read_timeout: Duration,
    total_timeout: Duration,
}
impl Default for GenerationRuntime {
    fn default() -> Self {
        Self::new()
    }
}
impl GenerationRuntime {
    pub fn new() -> Self {
        Self {
            connect_timeout: Duration::from_secs(4),
            read_timeout: Duration::from_secs(45),
            total_timeout: Duration::from_secs(180),
        }
    }

    /// Sends exactly one request. There is no retry, fallback, model download, or model change.
    pub async fn complete(
        &self,
        config: &GenerationConfig,
        messages: &[GenerationMessage],
        key: Option<&SecretValue>,
        cancellation: &GenerationCancellation,
    ) -> Result<GenerationResponse, GenerationError> {
        self.complete_with_response_schema(config, messages, key, cancellation, None)
            .await
    }

    /// Internal document-only schema, authored by Prism rather than accepted
    /// from frontend input. The caller separately opts in for a local server.
    /// An unsupported server remains an error: never retry without the schema.
    pub(crate) async fn complete_with_response_schema(
        &self,
        config: &GenerationConfig,
        messages: &[GenerationMessage],
        key: Option<&SecretValue>,
        cancellation: &GenerationCancellation,
        schema: Option<&Value>,
    ) -> Result<GenerationResponse, GenerationError> {
        cancellation.check()?;
        let endpoint = validate_endpoint(&config.endpoint)?;
        validate_request(config, messages)?;
        if schema.is_some() && !endpoint.is_local {
            return Err(error(GenerationErrorCode::Input));
        }
        if !endpoint.is_local && key.is_none() {
            return Err(error(GenerationErrorCode::Auth));
        }
        let body = match schema {
            Some(schema) => request_body_with_schema(config, messages, Some(schema))?,
            None => request_body(config, messages)?,
        };
        let client = self.client(&endpoint)?;
        let mut request = client
            .post(&endpoint.completion_url)
            .header(CONTENT_TYPE, "application/json")
            .body(body);
        if let Some(key) = key {
            let authorization = Zeroizing::new(format!("Bearer {}", key.expose()));
            let mut header = HeaderValue::from_bytes(authorization.as_bytes())
                .map_err(|_| error(GenerationErrorCode::Auth))?;
            header.set_sensitive(true);
            request = request.header(AUTHORIZATION, header);
        }
        let deadline = Instant::now() + self.total_timeout;
        let mut response = wait(request.send(), cancellation, deadline).await?;
        let status = response.status();
        // These statuses need no error body. In particular never expose server echoes.
        if let Some(code) = immediate_status_error(status) {
            return Err(error(code));
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
        {
            return Err(error(GenerationErrorCode::Malformed));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = wait(response.chunk(), cancellation, deadline).await? {
            if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
                return Err(error(GenerationErrorCode::Malformed));
            }
            bytes.extend_from_slice(&chunk);
        }
        cancellation.check()?;
        let value: Value =
            serde_json::from_slice(&bytes).map_err(|_| error(GenerationErrorCode::Malformed))?;
        if !status.is_success() || value.get("error").is_some_and(|item| !item.is_null()) {
            return Err(error(body_error_code(status, &value)));
        }
        let result = parse_response(value)?;
        cancellation.check()?;
        Ok(result)
    }

    fn client(&self, endpoint: &EndpointInfo) -> Result<Client, GenerationError> {
        let mut builder = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(self.connect_timeout)
            .read_timeout(self.read_timeout)
            .timeout(self.total_timeout)
            .user_agent("Prism/0.1 generation");
        let url = Url::parse(&endpoint.completion_url)
            .map_err(|_| error(GenerationErrorCode::Endpoint))?;
        if endpoint.is_local {
            builder = builder.no_proxy();
        }
        if endpoint.is_local && url.host_str() == Some("localhost") {
            // Pin localhost to loopback even if the machine's DNS/hosts configuration changes.
            let port = url
                .port_or_known_default()
                .ok_or_else(|| error(GenerationErrorCode::Endpoint))?;
            builder = builder.resolve_to_addrs(
                "localhost",
                &[
                    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port),
                    SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), port),
                ],
            );
        }
        builder
            .build()
            .map_err(|_| error(GenerationErrorCode::Network))
    }
}

async fn wait<T>(
    future: impl Future<Output = Result<T, reqwest::Error>>,
    cancellation: &GenerationCancellation,
    deadline: Instant,
) -> Result<T, GenerationError> {
    let mut future = Box::pin(future);
    loop {
        cancellation.check()?;
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .filter(|time| !time.is_zero())
            .ok_or_else(|| error(GenerationErrorCode::Network))?;
        match tokio::time::timeout(remaining.min(CANCEL_POLL), future.as_mut()).await {
            Ok(result) => {
                cancellation.check()?;
                return result.map_err(|_| error(GenerationErrorCode::Network));
            }
            Err(_) => continue,
        }
    }
}
fn validate_request(
    config: &GenerationConfig,
    messages: &[GenerationMessage],
) -> Result<(), GenerationError> {
    if config.model.trim().is_empty()
        || config.model.len() > 256
        || config.model.chars().any(char::is_control)
        || config.max_tokens == 0
        || config.max_tokens > 65_536
        || config
            .temperature
            .is_some_and(|value| !value.is_finite() || !(0.0..=2.0).contains(&value))
        || messages.is_empty()
        || messages.len() > 256
    {
        return Err(error(GenerationErrorCode::Input));
    }
    let bytes = messages.iter().try_fold(0usize, |total, message| {
        total.checked_add(message.content.len())
    });
    if bytes.is_none_or(|bytes| bytes > MAX_REQUEST_BYTES) {
        return Err(error(GenerationErrorCode::Input));
    }
    Ok(())
}
fn request_body(
    config: &GenerationConfig,
    messages: &[GenerationMessage],
) -> Result<Vec<u8>, GenerationError> {
    request_body_with_schema(config, messages, None)
}
fn request_body_with_schema(
    config: &GenerationConfig,
    messages: &[GenerationMessage],
    schema: Option<&Value>,
) -> Result<Vec<u8>, GenerationError> {
    let mut body =
        serde_json::json!({ "model": config.model, "messages": messages, "stream": false });
    let parameter = match config.token_parameter {
        TokenParameter::MaxTokens => "max_tokens",
        TokenParameter::MaxCompletionTokens => "max_completion_tokens",
    };
    body[parameter] = Value::from(config.max_tokens);
    if let Some(temperature) = config.temperature {
        body["temperature"] = Value::from(temperature as f64);
    }
    if let Some(schema) = schema {
        // This is a bounded, local document grammar, not a provider-supplied
        // schema or a URL to fetch. Endpoint/auth/cancellation guards still run.
        if !schema.is_object()
            || schema.get("type").and_then(Value::as_str) != Some("object")
            || serde_json::to_vec(schema)
                .map_err(|_| error(GenerationErrorCode::Input))?
                .len()
                > MAX_SCHEMA_BYTES
        {
            return Err(error(GenerationErrorCode::Input));
        }
        body["response_format"] = serde_json::json!({
            "type": "json_schema",
            "json_schema": { "name": "prism_document", "strict": true, "schema": schema }
        });
    }
    let bytes = serde_json::to_vec(&body).map_err(|_| error(GenerationErrorCode::Input))?;
    if bytes.len() > MAX_REQUEST_BYTES {
        return Err(error(GenerationErrorCode::Input));
    }
    Ok(bytes)
}
fn immediate_status_error(status: StatusCode) -> Option<GenerationErrorCode> {
    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Some(GenerationErrorCode::Auth),
        StatusCode::TOO_MANY_REQUESTS => Some(GenerationErrorCode::RateLimited),
        StatusCode::PAYLOAD_TOO_LARGE => Some(GenerationErrorCode::Context),
        StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED => {
            Some(GenerationErrorCode::Endpoint)
        }
        _ if status.is_redirection() => Some(GenerationErrorCode::Endpoint),
        _ if status.is_server_error() => Some(GenerationErrorCode::Server),
        _ => None,
    }
}
fn body_error_code(status: StatusCode, value: &Value) -> GenerationErrorCode {
    let item = value.get("error").unwrap_or(value);
    let code = item.get("code").and_then(Value::as_str).unwrap_or("");
    let kind = item.get("type").and_then(Value::as_str).unwrap_or("");
    let message = item
        .as_str()
        .or_else(|| item.get("message").and_then(Value::as_str))
        .unwrap_or("")
        .to_ascii_lowercase();
    if [code, kind].iter().any(|field| {
        field.contains("context_length")
            || field.contains("context_window")
            || field.contains("too_many_tokens")
    }) || message.contains("context length")
        || message.contains("context window")
        || message.contains("too many tokens")
    {
        GenerationErrorCode::Context
    } else if matches!(code, "invalid_api_key" | "authentication_error")
        || kind == "authentication_error"
    {
        GenerationErrorCode::Auth
    } else if matches!(code, "rate_limit_exceeded" | "insufficient_quota")
        || kind == "rate_limit_error"
    {
        GenerationErrorCode::RateLimited
    } else if status.is_client_error() {
        GenerationErrorCode::Input
    } else {
        GenerationErrorCode::Server
    }
}
fn parse_response(value: Value) -> Result<GenerationResponse, GenerationError> {
    let choice = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .ok_or_else(|| error(GenerationErrorCode::Malformed))?;
    let message = choice
        .get("message")
        .ok_or_else(|| error(GenerationErrorCode::Malformed))?;
    let text = message
        .get("content")
        .and_then(Value::as_str)
        .or_else(|| message.get("refusal").and_then(Value::as_str))
        .ok_or_else(|| error(GenerationErrorCode::Malformed))?;
    Ok(GenerationResponse {
        text: text.into(),
        model: value.get("model").and_then(Value::as_str).map(String::from),
        finish_reason: choice
            .get("finish_reason")
            .and_then(Value::as_str)
            .map(String::from),
        usage: value.get("usage").filter(|usage| !usage.is_null()).cloned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::mpsc,
        thread,
    };
    const FAKE_KEY: &str = "FAKE-KEY-DO-NOT-USE";
    const PROMPT: &str = "SYNTHETIC-PROMPT-CANARY";
    fn fixture_gate() -> std::sync::MutexGuard<'static, ()> {
        // Keep socket-based cancellation/deadline measurements independent of
        // simultaneous fixture work, including the oversized response case.
        static GATE: std::sync::Mutex<()> = std::sync::Mutex::new(());
        GATE.lock().unwrap_or_else(|error| error.into_inner())
    }
    fn runtime() -> &'static tokio::runtime::Runtime {
        static RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
        RUNTIME.get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap()
        })
    }
    fn config(endpoint: String) -> GenerationConfig {
        GenerationConfig {
            endpoint,
            model: "fixture-model".into(),
            max_tokens: 123,
            token_parameter: TokenParameter::MaxTokens,
            temperature: Some(0.25),
        }
    }
    fn messages() -> Vec<GenerationMessage> {
        vec![GenerationMessage {
            role: GenerationRole::User,
            content: PROMPT.into(),
        }]
    }
    fn fixture(
        status: u16,
        body: String,
        stall: bool,
        body_stall: bool,
    ) -> (
        String,
        mpsc::Receiver<(String, Value)>,
        thread::JoinHandle<()>,
    ) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, receiver) = mpsc::channel();
        let worker = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket.set_nodelay(true).unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut buffer = Vec::new();
            let mut chunk = [0u8; 1024];
            let header_end = loop {
                let count = socket.read(&mut chunk).unwrap();
                assert!(count > 0);
                buffer.extend_from_slice(&chunk[..count]);
                if let Some(end) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
                    break end + 4;
                }
                assert!(buffer.len() < 64 * 1024);
            };
            let headers = String::from_utf8(buffer[..header_end].to_vec()).unwrap();
            let length: usize = headers
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|value| value.trim().parse().unwrap())
                })
                .unwrap();
            while buffer.len() - header_end < length {
                let count = socket.read(&mut chunk).unwrap();
                assert!(count > 0);
                buffer.extend_from_slice(&chunk[..count]);
            }
            let request = serde_json::from_slice(&buffer[header_end..header_end + length]).unwrap();
            sender.send((headers, request)).unwrap();
            if stall {
                thread::sleep(Duration::from_millis(500));
                return;
            }
            let location = if (300..400).contains(&status) {
                "Location: http://127.0.0.1:9/should-not-connect\r\n"
            } else {
                ""
            };
            let head = format!("HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{location}Connection: keep-alive\r\n\r\n", body.len());
            if body_stall {
                let _ = socket.write_all(head.as_bytes());
                thread::sleep(Duration::from_millis(500));
                let _ = socket.write_all(body.as_bytes());
            } else {
                let mut reply = head.into_bytes();
                reply.extend_from_slice(body.as_bytes());
                let _ = socket.write_all(&reply);
            }
            // A persistent HTTP response is complete at Content-Length. Let the
            // client close its connection rather than racing a server-side FIN.
            let mut drain = [0u8; 1024];
            while matches!(socket.read(&mut drain), Ok(count) if count > 0) {}
        });
        (format!("http://{address}/v1"), receiver, worker)
    }
    fn answer() -> String {
        serde_json::json!({"model":"fixture-model","error":null,"choices":[{"message":{"role":"assistant","content":"합성 응답입니다."},"finish_reason":"stop"}],"usage":{"prompt_tokens":7,"completion_tokens":4,"total_tokens":11,"provider_detail":{"fixture":true}}}).to_string()
    }

    #[test]
    fn endpoint_validation_is_exact_and_never_accepts_credentials_or_remote_http() {
        for endpoint in [
            "http://127.0.0.1:1234/v1",
            "http://localhost:11434/v1/",
            "http://[::1]:1234/v1",
        ] {
            assert!(validate_endpoint(endpoint).unwrap().is_local);
        }
        for endpoint in [
            "http://127.1/v1",
            "http://2130706433/v1",
            "http://0x7f000001/v1",
            "http://127.0.0.2/v1",
            "http://localhost.evil/v1",
            "http://localhost./v1",
            "http://192.168.1.1/v1",
            "https://user:password@example.com/v1",
            "https://example.com/v1?api_key=FAKE",
            "https://example.com/v1#secret",
            "file:///etc/passwd",
            "http://@localhost/v1",
            "http://localhost:0/v1",
            "http://localhost\\@evil.example/v1",
            "http://localhost\n/v1",
        ] {
            assert!(validate_endpoint(endpoint).is_err(), "{endpoint}");
        }
        let remote = validate_endpoint("https://api.example.com/v1/chat/completions/").unwrap();
        assert!(!remote.is_local);
        assert_eq!(remote.base_url, "https://api.example.com/v1");
        assert_eq!(
            remote.completion_url,
            "https://api.example.com/v1/chat/completions"
        );
        assert!(!format!("{remote:?}").contains("api.example.com"));
        assert_eq!(
            validate_endpoint("http://localhost:1234")
                .unwrap()
                .completion_url,
            "http://localhost:1234/v1/chat/completions"
        );
    }
    #[test]
    fn exactly_the_selected_token_field_and_optional_temperature_are_sent() {
        let mut settings = config("http://localhost:1234/v1".into());
        let body: Value =
            serde_json::from_slice(&request_body(&settings, &messages()).unwrap()).unwrap();
        assert_eq!(body["max_tokens"], 123);
        assert!(body.get("max_completion_tokens").is_none());
        settings.token_parameter = TokenParameter::MaxCompletionTokens;
        settings.temperature = None;
        let body: Value =
            serde_json::from_slice(&request_body(&settings, &messages()).unwrap()).unwrap();
        assert_eq!(body["max_completion_tokens"], 123);
        assert!(body.get("max_tokens").is_none());
        assert!(body.get("temperature").is_none());
        settings.temperature = Some(f32::NAN);
        assert!(validate_request(&settings, &messages()).is_err());
    }
    #[test]
    fn document_schema_is_explicit_bounded_and_keeps_the_plain_request_unchanged() {
        let settings = config("http://localhost:1234/v1".into());
        let plain: Value =
            serde_json::from_slice(&request_body(&settings, &messages()).unwrap()).unwrap();
        assert!(plain.get("response_format").is_none());
        let schema = serde_json::json!({
            "type":"object","properties":{"blocks":{"type":"array"}},
            "required":["blocks"],"additionalProperties":false
        });
        let packet: Value = serde_json::from_slice(
            &request_body_with_schema(&settings, &messages(), Some(&schema)).unwrap(),
        )
        .unwrap();
        assert_eq!(packet["response_format"]["type"], "json_schema");
        assert_eq!(packet["response_format"]["json_schema"]["strict"], true);
        assert_eq!(packet["response_format"]["json_schema"]["schema"], schema);
        let mut without_schema = packet;
        without_schema
            .as_object_mut()
            .unwrap()
            .remove("response_format");
        assert_eq!(without_schema, plain);
        for invalid in [
            Value::Null,
            serde_json::json!({"type":"array"}),
            serde_json::json!({"type":"object","description":"x".repeat(MAX_SCHEMA_BYTES)}),
        ] {
            assert!(request_body_with_schema(&settings, &messages(), Some(&invalid)).is_err());
        }
    }
    #[test]
    fn local_server_gets_the_real_json_request_and_usage_is_preserved() {
        let _fixture = fixture_gate();
        let (endpoint, receiver, worker) = fixture(200, answer(), false, false);
        let key = SecretValue::new(FAKE_KEY.into()).unwrap();
        let result = runtime()
            .block_on(GenerationRuntime::new().complete(
                &config(endpoint),
                &messages(),
                Some(&key),
                &GenerationCancellation::new(),
            ))
            .unwrap();
        let (headers, request) = receiver.recv().unwrap();
        worker.join().unwrap();
        assert!(headers.starts_with("POST /v1/chat/completions HTTP/1.1"));
        assert!(headers.contains(FAKE_KEY));
        assert_eq!(request["model"], "fixture-model");
        assert_eq!(request["messages"][0]["content"], PROMPT);
        assert_eq!(request["stream"], false);
        assert_eq!(result.text, "합성 응답입니다.");
        assert_eq!(result.usage.unwrap()["provider_detail"]["fixture"], true);
    }
    #[test]
    fn document_schema_reaches_local_http_and_unsupported_response_is_an_error() {
        let _fixture = fixture_gate();
        let schema = serde_json::json!({
            "type":"object","properties":{"blocks":{"type":"array"}},
            "required":["blocks"],"additionalProperties":false
        });
        let (endpoint, receiver, worker) = fixture(
            400,
            r#"{"error":{"message":"response_format is unsupported"}}"#.into(),
            false,
            false,
        );
        let engine = GenerationRuntime::new();
        let cancellation = GenerationCancellation::new();
        let result = runtime().block_on(engine.complete_with_response_schema(
            &config(endpoint),
            &messages(),
            None,
            &cancellation,
            Some(&schema),
        ));
        assert_eq!(result.unwrap_err().code, GenerationErrorCode::Input);
        let (_, request) = receiver.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(request["response_format"]["json_schema"]["schema"], schema);
        worker.join().unwrap();
        let remote = config("https://example.invalid/v1".into());
        assert_eq!(
            runtime()
                .block_on(engine.complete_with_response_schema(
                    &remote,
                    &messages(),
                    None,
                    &cancellation,
                    Some(&schema),
                ))
                .unwrap_err()
                .code,
            GenerationErrorCode::Input
        );
    }
    #[test]
    fn usage_is_not_estimated_and_response_debug_is_redacted() {
        let response =
            parse_response(serde_json::json!({"choices":[{"message":{"content":PROMPT},"finish_reason":FAKE_KEY}]}))
                .unwrap();
        assert!(response.usage.is_none());
        assert!(!format!("{response:?}").contains(PROMPT));
        assert!(!format!("{response:?}").contains(FAKE_KEY));
        assert!(!format!("{:?}", messages()[0]).contains(PROMPT));
    }
    #[test]
    fn server_errors_and_malformed_bodies_never_echo_secrets_or_prompts() {
        let _fixture = fixture_gate();
        for (status,body,expected) in [
            (401,format!("{{\"error\":\"{FAKE_KEY} {PROMPT}\"}}"),GenerationErrorCode::Auth),
            (429,"{}".into(),GenerationErrorCode::RateLimited),
            (500,format!("{FAKE_KEY} {PROMPT}"),GenerationErrorCode::Server),
            (400,serde_json::json!({"error":{"code":"context_length_exceeded","message":format!("{FAKE_KEY} {PROMPT}")}}).to_string(),GenerationErrorCode::Context),
            (200,format!("invalid JSON {FAKE_KEY} {PROMPT}"),GenerationErrorCode::Malformed),
            (302,"{}".into(),GenerationErrorCode::Endpoint),
        ] {
            let (endpoint,receiver,worker) = fixture(status,body,false,false);
            let key = SecretValue::new(FAKE_KEY.into()).unwrap();
            let failure = runtime().block_on(GenerationRuntime::new().complete(&config(endpoint),&messages(),Some(&key),&GenerationCancellation::new())).unwrap_err();
            receiver.recv().unwrap(); worker.join().unwrap();
            assert_eq!(failure.code,expected);
            let exposed = format!("{failure:?} {}",serde_json::to_string(&failure).unwrap());
            assert!(!exposed.contains(FAKE_KEY) && !exposed.contains(PROMPT));
        }
    }
    #[test]
    fn cancellation_interrupts_header_and_body_stalls() {
        let _fixture = fixture_gate();
        for body_stall in [false, true] {
            let (endpoint, receiver, worker) = fixture(200, answer(), !body_stall, body_stall);
            let cancellation = GenerationCancellation::new();
            let trigger = cancellation.clone();
            // Measure from the moment cancel is pressed, not from the start of the request:
            // client/runtime setup time on a busy CI machine is not cancellation latency.
            let (cancelled_at, cancelled) = std::sync::mpsc::channel();
            let cancel_worker = thread::spawn(move || {
                receiver.recv().unwrap();
                cancelled_at.send(Instant::now()).unwrap();
                trigger.cancel();
            });
            let failure = runtime()
                .block_on(GenerationRuntime::new().complete(
                    &config(endpoint),
                    &messages(),
                    None,
                    &cancellation,
                ))
                .unwrap_err();
            assert_eq!(failure.code, GenerationErrorCode::Cancelled);
            // The stall would otherwise last until the 45 s read timeout, so a quick return
            // here can only come from cancellation.
            let pressed = cancelled.recv().unwrap();
            assert!(pressed.elapsed() < Duration::from_millis(450));
            cancel_worker.join().unwrap();
            worker.join().unwrap();
        }
    }
    #[test]
    fn response_limit_read_and_total_timeouts_are_enforced() {
        let _fixture = fixture_gate();
        let (endpoint, receiver, worker) =
            fixture(200, "x".repeat(MAX_RESPONSE_BYTES + 1), false, false);
        let failure = runtime()
            .block_on(GenerationRuntime::new().complete(
                &config(endpoint),
                &messages(),
                None,
                &GenerationCancellation::new(),
            ))
            .unwrap_err();
        receiver.recv().unwrap();
        worker.join().unwrap();
        assert_eq!(failure.code, GenerationErrorCode::Malformed);
        let (endpoint, receiver, worker) = fixture(200, answer(), false, true);
        let mut transport = GenerationRuntime::new();
        transport.read_timeout = Duration::from_millis(150);
        let failure = runtime()
            .block_on(transport.complete(
                &config(endpoint),
                &messages(),
                None,
                &GenerationCancellation::new(),
            ))
            .unwrap_err();
        receiver.recv().unwrap();
        worker.join().unwrap();
        assert_eq!(failure.code, GenerationErrorCode::Network);
        let (endpoint, receiver, worker) = fixture(200, answer(), false, true);
        let mut transport = GenerationRuntime::new();
        transport.total_timeout = Duration::from_millis(125);
        let started = Instant::now();
        let failure = runtime()
            .block_on(transport.complete(
                &config(endpoint),
                &messages(),
                None,
                &GenerationCancellation::new(),
            ))
            .unwrap_err();
        assert_eq!(failure.code, GenerationErrorCode::Network);
        assert!(started.elapsed() < Duration::from_millis(450));
        receiver.recv().unwrap();
        worker.join().unwrap();
    }
    #[test]
    fn pre_cancelled_and_missing_remote_key_do_not_send_requests() {
        let cancellation = GenerationCancellation::new();
        cancellation.cancel();
        assert_eq!(
            runtime()
                .block_on(GenerationRuntime::new().complete(
                    &config("https://api.example.com/v1".into()),
                    &messages(),
                    None,
                    &cancellation
                ))
                .unwrap_err()
                .code,
            GenerationErrorCode::Cancelled
        );
        assert_eq!(
            runtime()
                .block_on(GenerationRuntime::new().complete(
                    &config("https://api.example.com/v1".into()),
                    &messages(),
                    None,
                    &GenerationCancellation::new()
                ))
                .unwrap_err()
                .code,
            GenerationErrorCode::Auth
        );
    }
}
