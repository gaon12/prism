//! App-owned EmbeddingGemma 2 CPU server. No document is sent to a remote service.
//! Public methods block: invoke them from a worker, never the WebView/UI thread.
pub use crate::embedding_assets::RUNTIME_VERSION;
use crate::embedding_assets::{self, Asset};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{Cursor, Read, Write},
    net::TcpListener,
    path::{Component, Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex, MutexGuard,
    },
    thread,
    time::{Duration, Instant},
};

pub const MODEL_ID: &str = "google/embeddinggemma-2";
pub const MODEL_REVISION: &str = "6ea3e3a309d12a4779cfda98b8c19800d2d5da71";
pub const NATIVE_DIMENSIONS: usize = 768;
const PREFIX_VERSION: &str = "search-result-document-v1";
const CANCELLED: &str = "Embedding operation cancelled";
const MAX_INPUT_BYTES: usize = 24_000;
const MAX_DOCUMENTS: usize = 64;
const MAX_RESPONSE_BYTES: u64 = 64 * 1024;

const MODEL: Asset = Asset {
    filename: "embeddinggemma-2-Q8_0.gguf",
    url: "https://huggingface.co/ggml-org/embeddinggemma-2-GGUF/resolve/6ea3e3a309d12a4779cfda98b8c19800d2d5da71/embeddinggemma-2-Q8_0.gguf",
    bytes: 309_855_456,
    sha256: "2188ac1deca4b77dffefd603c2776a9d76d9d74ec01841392982ebb840b09135",
};
const BF16: Asset = Asset {
    filename: "embeddinggemma-2-BF16.gguf",
    url: "",
    bytes: 557_950_176,
    sha256: "68bae29d62fb8c7d23e98d21fd4662753ddd636e6b62b8a70dfe059a9844f216",
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum EmbeddingPhase {
    NotInstalled,
    Downloading,
    Ready,
    Starting,
    Running,
    Cancelled,
    Error,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingStatus {
    pub phase: EmbeddingPhase,
    pub model_id: String,
    pub model_revision: String,
    pub runtime_version: String,
    pub runtime_target: Option<String>,
    pub runtime_filename: Option<String>,
    pub runtime_bytes: Option<u64>,
    pub quantization: Option<String>,
    pub model_ready: bool,
    pub runtime_ready: bool,
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    pub pid: Option<u32>,
    pub dimensions: usize,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingDocument {
    pub title: Option<String>,
    pub text: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelInstall {
    revision: String,
    filename: String,
    sha256: String,
    quantization: String,
}
impl ModelInstall {
    fn asset(&self) -> Option<Asset> {
        [MODEL, BF16].into_iter().find(|asset| {
            self.revision == MODEL_REVISION
                && self.filename == asset.filename
                && self.sha256 == asset.sha256
                && self.quantization
                    == if asset.filename == MODEL.filename {
                        "Q8_0"
                    } else {
                        "BF16"
                    }
        })
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeInstall {
    version: String,
    directory: String,
    #[serde(default)]
    target: Option<String>,
}
impl RuntimeInstall {
    fn valid(&self) -> bool {
        self.version == RUNTIME_VERSION
            && self.directory.starts_with("runtime-b11960-")
            && safe_flat_name(Path::new(&self.directory))
            && embedding_assets::host().is_ok_and(|package| match &self.target {
                Some(target) => target == package.target,
                // Existing Windows installs predate the target field.
                None => package.target == "windows-x86_64",
            })
    }
}

struct ManagedServer {
    child: Child,
    endpoint: String,
    token: String,
    // Closing this private job also reaps the server if Prism exits abnormally.
    _job: crate::process_job::OwnedJobGuard,
}
impl Drop for ManagedServer {
    fn drop(&mut self) {
        // Only the Child created by this instance is terminated. Never kill by process name.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
struct Inner {
    root: PathBuf,
    state: Mutex<EmbeddingStatus>,
    model: Mutex<Option<ModelInstall>>,
    runtime: Mutex<Option<RuntimeInstall>>,
    operation: Mutex<()>,
    inference: Mutex<()>,
    cancel: Arc<AtomicBool>,
    server: Mutex<Option<ManagedServer>>,
}
#[derive(Clone)]
pub struct EmbeddingRuntime {
    inner: Arc<Inner>,
}
fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|error| error.into_inner())
}

impl EmbeddingRuntime {
    pub fn new(data_dir: PathBuf) -> Result<Self, String> {
        let root = data_dir.join("embedding");
        fs::create_dir_all(&root).map_err(|_| "Cannot create embedding storage".to_string())?;
        let model = read_manifest::<ModelInstall>(&root.join("model.json"))
            .filter(|item| item.asset().is_some());
        let runtime =
            read_manifest::<RuntimeInstall>(&root.join("runtime.json")).filter(|item| item.valid());
        let service = Self {
            inner: Arc::new(Inner {
                root,
                model: Mutex::new(model),
                runtime: Mutex::new(runtime),
                operation: Mutex::new(()),
                inference: Mutex::new(()),
                cancel: Arc::new(AtomicBool::new(false)),
                server: Mutex::new(None),
                state: Mutex::new(EmbeddingStatus {
                    phase: EmbeddingPhase::NotInstalled,
                    model_id: MODEL_ID.into(),
                    model_revision: MODEL_REVISION.into(),
                    runtime_version: RUNTIME_VERSION.into(),
                    runtime_target: embedding_assets::host()
                        .ok()
                        .map(|package| package.target.into()),
                    runtime_filename: embedding_assets::host()
                        .ok()
                        .map(|package| package.asset.filename.into()),
                    runtime_bytes: embedding_assets::host()
                        .ok()
                        .map(|package| package.asset.bytes),
                    quantization: None,
                    model_ready: false,
                    runtime_ready: false,
                    downloaded_bytes: 0,
                    total_bytes: 0,
                    pid: None,
                    dimensions: NATIVE_DIMENSIONS,
                    last_error: None,
                }),
            }),
        };
        service.refresh_assets();
        Ok(service)
    }

    pub fn status(&self) -> EmbeddingStatus {
        let mut server = lock(&self.inner.server);
        if let Some(owned) = server.as_mut() {
            if !matches!(owned.child.try_wait(), Ok(None)) {
                *server = None;
                let mut state = lock(&self.inner.state);
                state.pid = None;
                state.phase = EmbeddingPhase::Error;
                state.last_error = Some("Embedding server exited; restart it to retry".into());
            }
        }
        lock(&self.inner.state).clone()
    }

    /// Cancels download/import/start at a cooperative boundary; polling status stays nonblocking.
    pub fn cancel_download(&self) {
        self.inner.cancel.store(true, Ordering::Release);
    }

    pub fn download_assets(&self) -> Result<EmbeddingStatus, String> {
        let runtime = embedding_assets::host()?.asset;
        self.run_operation(EmbeddingPhase::Downloading, || {
            self.ensure_stopped()?;
            {
                let mut state = lock(&self.inner.state);
                state.downloaded_bytes = 0;
                state.total_bytes = MODEL.bytes + runtime.bytes;
            }
            self.download_asset(MODEL)?;
            self.register_model(MODEL)?;
            self.download_asset(runtime)?;
            self.install_runtime(&self.inner.root.join(runtime.filename))
        })
    }

    pub fn import_model(&self, path: &Path) -> Result<EmbeddingStatus, String> {
        self.run_operation(EmbeddingPhase::Downloading, || {
            self.ensure_stopped()?;
            if !path.is_absolute() {
                return Err("Choose an absolute GGUF file path".into());
            }
            let bytes = fs::metadata(path)
                .map_err(|_| "Cannot read selected model".to_string())?
                .len();
            let asset = [MODEL, BF16]
                .into_iter()
                .find(|asset| asset.bytes == bytes)
                .ok_or(
                    "Unsupported model: choose the official EmbeddingGemma 2 Q8_0 or BF16 GGUF",
                )?;
            self.copy_asset(path, asset)?;
            self.register_model(asset)
        })
    }

    pub fn import_runtime(&self, path: &Path) -> Result<EmbeddingStatus, String> {
        let runtime = embedding_assets::host()?.asset;
        self.run_operation(EmbeddingPhase::Downloading, || {
            self.ensure_stopped()?;
            if !path.is_absolute() {
                return Err("Choose an absolute CPU runtime archive path".into());
            }
            self.copy_asset(path, runtime)?;
            self.install_runtime(&self.inner.root.join(runtime.filename))
        })
    }

    pub fn start(&self) -> Result<EmbeddingStatus, String> {
        let package = embedding_assets::host()?;
        if self.status().phase == EmbeddingPhase::Running {
            return Ok(self.status());
        }
        self.run_operation(EmbeddingPhase::Starting, || {
            self.ensure_stopped()?;
            let model = lock(&self.inner.model)
                .clone()
                .ok_or("Download or import EmbeddingGemma 2 first")?;
            let asset = model.asset().ok_or("Unsupported embedding model version")?;
            self.verify_asset(&self.inner.root.join(asset.filename), asset)?;
            let runtime = lock(&self.inner.runtime)
                .clone()
                .filter(|item| item.valid())
                .ok_or("Download or import the CPU runtime for this platform first")?;
            let runtime_dir = self.inner.root.join(&runtime.directory);
            self.verify_runtime(&runtime_dir)?;
            let client = local_client(Duration::from_secs(2))?;
            for _attempt in 0..3 {
                self.check_cancel()?;
                let reservation = TcpListener::bind(("127.0.0.1", 0))
                    .map_err(|_| "Cannot reserve local embedding port".to_string())?;
                let port = reservation
                    .local_addr()
                    .map_err(|_| "Cannot inspect local embedding port".to_string())?
                    .port();
                let endpoint = format!("http://127.0.0.1:{port}");
                let token = random_hex()?;
                let mut command = Command::new(runtime_dir.join(package.executable));
                command
                    .current_dir(&runtime_dir)
                    .args([
                        "--host",
                        "127.0.0.1",
                        "--port",
                        &port.to_string(),
                        "--model",
                    ])
                    .arg(self.inner.root.join(asset.filename))
                    .args([
                        "--embedding",
                        "--pooling",
                        "mean",
                        "--ctx-size",
                        "8192",
                        "--batch-size",
                        "8192",
                        "--ubatch-size",
                        "8192",
                        "--parallel",
                        "1",
                        "--threads",
                        "4",
                        "--threads-batch",
                        "4",
                        "--n-gpu-layers",
                        "0",
                        "--no-mmproj",
                        "--log-disable",
                    ])
                    .env("LLAMA_API_KEY", &token)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());
                #[cfg(windows)]
                {
                    use std::os::windows::process::CommandExt;
                    command.creation_flags(0x08000000); // CREATE_NO_WINDOW
                }
                drop(reservation);
                let job = crate::process_job::OwnedJobGuard::new()
                    .map_err(|_| "Cannot create the embedding process owner".to_string())?;
                let mut child = command.spawn().map_err(|_| {
                    "Cannot launch embedding CPU runtime; check this platform's runtime dependencies"
                        .to_string()
                })?;
                if job.assign(&child).is_err() {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("Cannot bind the embedding server to this app process".into());
                }
                let mut owned = ManagedServer {
                    child,
                    endpoint,
                    token,
                    _job: job,
                };
                let deadline = Instant::now() + Duration::from_secs(90);
                loop {
                    self.check_cancel()?;
                    if !matches!(owned.child.try_wait(), Ok(None)) {
                        break;
                    }
                    if let Ok(response) = client
                        .get(format!("{}/health", owned.endpoint))
                        .bearer_auth(&owned.token)
                        .send()
                    {
                        if response.status().is_success() {
                            // Require authenticated identity, not another server winning the port race.
                            if self.probe_identity(&owned)? {
                                let pid = owned.child.id();
                                *lock(&self.inner.server) = Some(owned);
                                let mut state = lock(&self.inner.state);
                                state.pid = Some(pid);
                                state.phase = EmbeddingPhase::Running;
                                return Ok(());
                            }
                        }
                    }
                    if Instant::now() >= deadline {
                        return Err("Embedding CPU server startup timed out".into());
                    }
                    thread::sleep(Duration::from_millis(150));
                }
                // A failed process (including a port race) is reaped by ManagedServer::drop.
            }
            Err("Embedding CPU server exited during startup".into())
        })
    }

    pub fn stop(&self) -> Result<EmbeddingStatus, String> {
        self.cancel_download();
        let _operation = lock(&self.inner.operation);
        *lock(&self.inner.server) = None;
        self.refresh_assets();
        let mut state = lock(&self.inner.state);
        state.pid = None;
        state.phase = if state.model_ready && state.runtime_ready {
            EmbeddingPhase::Ready
        } else {
            EmbeddingPhase::NotInstalled
        };
        state.last_error = None;
        Ok(state.clone())
    }

    pub fn embed_query(&self, text: &str, dimensions: usize) -> Result<Vec<f32>, String> {
        validate_dimensions(dimensions)?;
        validate_input(text)?;
        self.embed(&format!("task: search result | query: {text}"), dimensions)
    }

    pub fn embed_documents(
        &self,
        documents: &[EmbeddingDocument],
        dimensions: usize,
    ) -> Result<Vec<Vec<f32>>, String> {
        validate_dimensions(dimensions)?;
        if documents.len() > MAX_DOCUMENTS {
            return Err("Embedding batch exceeds 64 documents".into());
        }
        // Validate the whole batch before doing work. One HTTP input per request avoids a
        // reproducible b11960 Windows response stall for responses around 64 KiB.
        let inputs: Vec<String> = documents
            .iter()
            .map(document_input)
            .collect::<Result<_, _>>()?;
        inputs
            .iter()
            .map(|input| self.embed(input, dimensions))
            .collect()
    }

    pub fn fingerprint(&self, dimensions: usize) -> Result<String, String> {
        validate_dimensions(dimensions)?;
        let model = lock(&self.inner.model)
            .clone()
            .ok_or("No embedding model installed")?;
        Ok(format!(
            "{MODEL_ID}:{MODEL_REVISION}:{}:{PREFIX_VERSION}:{dimensions}",
            model.sha256
        ))
    }

    fn embed(&self, input: &str, dimensions: usize) -> Result<Vec<f32>, String> {
        let _inference = lock(&self.inner.inference);
        let (endpoint, token) = {
            let mut guard = lock(&self.inner.server);
            let server = guard
                .as_mut()
                .ok_or("Start EmbeddingGemma 2 before searching")?;
            if !matches!(server.child.try_wait(), Ok(None)) {
                return Err("Embedding server is no longer running".into());
            }
            (server.endpoint.clone(), server.token.clone())
        };
        let response = local_client(Duration::from_secs(60))?.post(format!("{endpoint}/v1/embeddings"))
            .bearer_auth(token).json(&serde_json::json!({ "model": "embeddinggemma-2", "input": [input], "encoding_format": "float" }))
            .send().map_err(|_| "Local embedding request failed or timed out".to_string())?;
        if !response.status().is_success() {
            return Err("Local embedding server rejected the input; shorten the passage or restart the server".into());
        }
        let body = bounded_body(response)?;
        #[derive(Deserialize)]
        struct Row {
            index: usize,
            embedding: Vec<f32>,
        }
        #[derive(Deserialize)]
        struct Response {
            data: Vec<Row>,
        }
        let mut response: Response =
            serde_json::from_slice(&body).map_err(|_| "Invalid embedding response".to_string())?;
        if response.data.len() != 1 || response.data[0].index != 0 {
            return Err("Unexpected embedding response count".into());
        }
        normalize(response.data.remove(0).embedding, dimensions)
    }

    fn probe_identity(&self, owned: &ManagedServer) -> Result<bool, String> {
        let response = local_client(Duration::from_secs(2))?
            .get(format!("{}/v1/models", owned.endpoint))
            .bearer_auth(&owned.token)
            .send();
        match response {
            Ok(response) => Ok(response.status().is_success()),
            Err(_) => Ok(false),
        }
    }

    fn run_operation(
        &self,
        phase: EmbeddingPhase,
        work: impl FnOnce() -> Result<(), String>,
    ) -> Result<EmbeddingStatus, String> {
        let _operation = self
            .inner
            .operation
            .try_lock()
            .map_err(|_| "Another embedding operation is already running".to_string())?;
        // A rejected asset mutation must not turn a healthy running server into Error.
        if phase == EmbeddingPhase::Downloading && lock(&self.inner.server).is_some() {
            return Err("Stop the embedding server before changing its assets".into());
        }
        self.inner.cancel.store(false, Ordering::Release);
        {
            let mut state = lock(&self.inner.state);
            state.phase = phase;
            state.last_error = None;
        }
        let result = work();
        self.refresh_assets();
        let mut state = lock(&self.inner.state);
        match result {
            Ok(()) => {
                state.phase = if state.pid.is_some() {
                    EmbeddingPhase::Running
                } else if state.model_ready && state.runtime_ready {
                    EmbeddingPhase::Ready
                } else {
                    EmbeddingPhase::NotInstalled
                };
                Ok(state.clone())
            }
            Err(error) => {
                state.phase = if error == CANCELLED {
                    EmbeddingPhase::Cancelled
                } else {
                    EmbeddingPhase::Error
                };
                state.last_error = Some(error.clone());
                Err(error)
            }
        }
    }

    fn check_cancel(&self) -> Result<(), String> {
        if self.inner.cancel.load(Ordering::Acquire) {
            Err(CANCELLED.into())
        } else {
            Ok(())
        }
    }
    fn ensure_stopped(&self) -> Result<(), String> {
        if lock(&self.inner.server).is_some() {
            Err("Stop the embedding server before changing its assets".into())
        } else {
            Ok(())
        }
    }

    fn refresh_assets(&self) {
        let model = lock(&self.inner.model).clone();
        let runtime = lock(&self.inner.runtime).clone();
        let model_ready = model
            .as_ref()
            .and_then(ModelInstall::asset)
            .is_some_and(|asset| has_size(&self.inner.root.join(asset.filename), asset.bytes));
        let runtime_ready = runtime.as_ref().is_some_and(|item| {
            item.valid()
                && self
                    .inner
                    .root
                    .join(&item.directory)
                    .join(
                        embedding_assets::host()
                            .map(|p| p.executable)
                            .unwrap_or("unsupported"),
                    )
                    .is_file()
                && embedding_assets::host().is_ok_and(|package| {
                    has_size(
                        &self.inner.root.join(package.asset.filename),
                        package.asset.bytes,
                    )
                })
        });
        let mut state = lock(&self.inner.state);
        state.model_ready = model_ready;
        state.runtime_ready = runtime_ready;
        state.quantization = model.filter(|_| model_ready).map(|item| item.quantization);
        if state.phase == EmbeddingPhase::NotInstalled && model_ready && runtime_ready {
            state.phase = EmbeddingPhase::Ready;
        }
    }

    fn register_model(&self, asset: Asset) -> Result<(), String> {
        let model = ModelInstall {
            revision: MODEL_REVISION.into(),
            filename: asset.filename.into(),
            sha256: asset.sha256.into(),
            quantization: if asset.filename == MODEL.filename {
                "Q8_0"
            } else {
                "BF16"
            }
            .into(),
        };
        write_manifest(&self.inner.root.join("model.json"), &model)?;
        *lock(&self.inner.model) = Some(model);
        Ok(())
    }

    fn verify_asset(&self, path: &Path, asset: Asset) -> Result<(), String> {
        if !has_size(path, asset.bytes) {
            return Err(format!(
                "{} is missing or has the wrong size",
                asset.filename
            ));
        }
        let file = File::open(path).map_err(|_| "Cannot open embedding asset".to_string())?;
        if self.hash_reader(file)? != asset.sha256 {
            return Err(format!(
                "{} SHA-256 mismatch; download or import it again",
                asset.filename
            ));
        }
        Ok(())
    }

    fn hash_reader(&self, mut reader: impl Read) -> Result<String, String> {
        let mut hash = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        loop {
            self.check_cancel()?;
            let count = reader
                .read(&mut buffer)
                .map_err(|_| "Cannot read embedding asset".to_string())?;
            if count == 0 {
                break;
            }
            hash.update(&buffer[..count]);
        }
        Ok(format!("{:x}", hash.finalize()))
    }

    fn download_asset(&self, asset: Asset) -> Result<(), String> {
        let destination = self.inner.root.join(asset.filename);
        if self.verify_asset(&destination, asset).is_ok() {
            lock(&self.inner.state).downloaded_bytes += asset.bytes;
            return Ok(());
        }
        self.check_cancel()?;
        // Keep stalled HTTPS reads outside the serialized service operation. Cancellation
        // closes the receiver within 100 ms; the bounded worker exits on its next send or
        // HTTP timeout and never owns a service handle or user document.
        let reader = DownloadReader::start(asset, Arc::clone(&self.inner.cancel))?;
        self.store_asset(reader, asset)
    }

    fn copy_asset(&self, source: &Path, asset: Asset) -> Result<(), String> {
        {
            let mut state = lock(&self.inner.state);
            state.downloaded_bytes = 0;
            state.total_bytes = asset.bytes;
        }
        let file =
            File::open(source).map_err(|_| "Cannot read selected embedding asset".to_string())?;
        self.store_asset(file, asset)
    }

    fn store_asset(&self, mut reader: impl Read, asset: Asset) -> Result<(), String> {
        let part = self
            .inner
            .root
            .join(format!("{}.{}.part", asset.filename, random_hex()?));
        let result = (|| {
            let mut file = File::create(&part)
                .map_err(|_| "Cannot create embedding download file".to_string())?;
            let mut hash = Sha256::new();
            let mut total = 0u64;
            let mut buffer = [0u8; 64 * 1024];
            loop {
                self.check_cancel()?;
                let count = reader
                    .read(&mut buffer)
                    .map_err(|_| if self.inner.cancel.load(Ordering::Acquire) {
                        CANCELLED.to_string()
                    } else {
                        "Embedding download or import was interrupted; retry or import a verified file".to_string()
                    })?;
                if count == 0 {
                    break;
                }
                total += count as u64;
                if total > asset.bytes {
                    return Err("Embedding asset exceeds the pinned size".into());
                }
                file.write_all(&buffer[..count]).map_err(|_| {
                    "Cannot save embedding asset; check available disk space".to_string()
                })?;
                hash.update(&buffer[..count]);
                lock(&self.inner.state).downloaded_bytes += count as u64;
            }
            if total != asset.bytes || format!("{:x}", hash.finalize()) != asset.sha256 {
                return Err("Embedding asset SHA-256 or size mismatch".into());
            }
            self.check_cancel()?;
            file.sync_all()
                .map_err(|_| "Cannot flush embedding asset".to_string())?;
            drop(file);
            replace_file(&part, &self.inner.root.join(asset.filename))
        })();
        if result.is_err() {
            let _ = fs::remove_file(&part);
        }
        result
    }

    fn install_runtime(&self, archive: &Path) -> Result<(), String> {
        let package = embedding_assets::host()?;
        self.verify_asset(archive, package.asset)?;
        let installed = lock(&self.inner.runtime)
            .clone()
            .filter(RuntimeInstall::valid);
        if let Some(installed) = installed {
            if self
                .verify_runtime(&self.inner.root.join(&installed.directory))
                .is_ok()
            {
                return Ok(());
            }
            self.check_cancel()?;
        }
        let directory = format!("runtime-b11960-{}", random_hex()?);
        let destination = self.inner.root.join(&directory);
        fs::create_dir(&destination).map_err(|_| "Cannot create runtime directory".to_string())?;
        let result = (|| {
            crate::embedding_package::inspect(archive, &destination, package, true, &|| {
                self.check_cancel()
            })?;
            self.verify_runtime(&destination)?;
            let install = RuntimeInstall {
                version: RUNTIME_VERSION.into(),
                directory,
                target: Some(package.target.into()),
            };
            write_manifest(&self.inner.root.join("runtime.json"), &install)?;
            *lock(&self.inner.runtime) = Some(install);
            Ok(())
        })();
        if result.is_err() && destination.parent() == Some(self.inner.root.as_path()) {
            // A generated, private child; package extraction never creates links.
            let _ = fs::remove_dir_all(&destination);
        }
        result
    }

    fn verify_runtime(&self, directory: &Path) -> Result<(), String> {
        let package = embedding_assets::host()?;
        let archive = self.inner.root.join(package.asset.filename);
        self.verify_asset(&archive, package.asset)?;
        crate::embedding_package::inspect(&archive, directory, package, false, &|| {
            self.check_cancel()
        })
    }
}

struct DownloadReader {
    chunks: mpsc::Receiver<Result<Vec<u8>, String>>,
    pending: Cursor<Vec<u8>>,
    cancel: Arc<AtomicBool>,
}
impl DownloadReader {
    fn start(asset: Asset, cancel: Arc<AtomicBool>) -> Result<Self, String> {
        let (sender, chunks) = mpsc::sync_channel(2);
        let worker_cancel = Arc::clone(&cancel);
        thread::Builder::new()
            .name("prism-model-download".into())
            .spawn(move || {
                let result = (|| -> Result<(), String> {
                    let client = Client::builder()
                        .https_only(true)
                        .connect_timeout(Duration::from_secs(10))
                        .timeout(Duration::from_secs(600))
                        .build()
                        .map_err(|_| "Cannot initialize HTTPS downloader".to_string())?;
                    let mut response = client
                        .get(asset.url)
                        .send()
                        .map_err(|_| "Embedding asset download failed".to_string())?;
                    if !response.status().is_success()
                        || response
                            .content_length()
                            .is_some_and(|length| length != asset.bytes)
                    {
                        return Err("Embedding asset release response is invalid".into());
                    }
                    loop {
                        if worker_cancel.load(Ordering::Acquire) {
                            return Err(CANCELLED.into());
                        }
                        let mut chunk = vec![0u8; 64 * 1024];
                        let bytes = response
                            .read(&mut chunk)
                            .map_err(|_| "Embedding asset download interrupted".to_string())?;
                        chunk.truncate(bytes);
                        sender.send(Ok(chunk)).map_err(|_| CANCELLED.to_string())?;
                        if bytes == 0 {
                            return Ok(());
                        }
                    }
                })();
                if let Err(error) = result {
                    let _ = sender.send(Err(error));
                }
            })
            .map_err(|_| "Cannot start embedding downloader".to_string())?;
        Ok(Self {
            chunks,
            pending: Cursor::new(Vec::new()),
            cancel,
        })
    }
}
impl Read for DownloadReader {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        loop {
            if self.cancel.load(Ordering::Acquire) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    CANCELLED,
                ));
            }
            let bytes = self.pending.read(output)?;
            if bytes != 0 {
                return Ok(bytes);
            }
            match self.chunks.recv_timeout(Duration::from_millis(100)) {
                Ok(Ok(chunk)) if chunk.is_empty() => return Ok(0),
                Ok(Ok(chunk)) => self.pending = Cursor::new(chunk),
                Ok(Err(error)) => return Err(std::io::Error::other(error)),
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "Embedding download worker stopped",
                    ))
                }
            }
        }
    }
}

fn local_client(timeout: Duration) -> Result<Client, String> {
    Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(2))
        .timeout(timeout)
        .build()
        .map_err(|_| "Cannot initialize local embedding connection".to_string())
}
fn bounded_body(response: reqwest::blocking::Response) -> Result<Vec<u8>, String> {
    if response
        .content_length()
        .is_some_and(|bytes| bytes > MAX_RESPONSE_BYTES)
    {
        return Err("Embedding response exceeds its size limit".into());
    }
    let mut body = Vec::new();
    response
        .take(MAX_RESPONSE_BYTES + 1)
        .read_to_end(&mut body)
        .map_err(|_| "Embedding response was interrupted".to_string())?;
    if body.len() as u64 > MAX_RESPONSE_BYTES {
        return Err("Embedding response exceeds its size limit".into());
    }
    Ok(body)
}
fn validate_dimensions(dimensions: usize) -> Result<(), String> {
    if [128, 256, 512, 768].contains(&dimensions) {
        Ok(())
    } else {
        Err("Embedding dimensions must be 128, 256, 512, or 768".into())
    }
}
fn validate_input(text: &str) -> Result<(), String> {
    if text.trim().is_empty() {
        return Err("Embedding input is empty".into());
    }
    if text.len() > MAX_INPUT_BYTES {
        return Err("Embedding input exceeds 24000 UTF-8 bytes; split it into passages".into());
    }
    Ok(())
}
fn document_input(document: &EmbeddingDocument) -> Result<String, String> {
    validate_input(&document.text)?;
    let title = document
        .title
        .as_deref()
        .filter(|title| !title.trim().is_empty())
        .unwrap_or("none");
    if title.len() > 2048 {
        return Err("Embedding document title is too long".into());
    }
    let input = format!("title: {title} | text: {}", document.text);
    validate_input(&input)?;
    Ok(input)
}
fn normalize(mut vector: Vec<f32>, dimensions: usize) -> Result<Vec<f32>, String> {
    validate_dimensions(dimensions)?;
    if vector.len() != NATIVE_DIMENSIONS || vector.iter().any(|value| !value.is_finite()) {
        return Err("Embedding server returned invalid dimensions or numbers".into());
    }
    vector.truncate(dimensions);
    let norm = vector
        .iter()
        .map(|&value| (value as f64).powi(2))
        .sum::<f64>()
        .sqrt();
    if !norm.is_finite() || norm <= 1e-12 {
        return Err("Embedding server returned a zero vector".into());
    }
    vector
        .iter_mut()
        .for_each(|value| *value = (*value as f64 / norm) as f32);
    Ok(vector)
}
fn safe_flat_name(path: &Path) -> bool {
    let mut components = path.components();
    matches!(components.next(), Some(Component::Normal(_)))
        && components.next().is_none()
        && !path.to_string_lossy().contains([':', '\\', '/'])
}
fn has_size(path: &Path, bytes: u64) -> bool {
    fs::metadata(path).is_ok_and(|metadata| metadata.is_file() && metadata.len() == bytes)
}
fn random_hex() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|_| "Cannot initialize embedding runtime randomness".to_string())?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}
fn read_manifest<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    let file = File::open(path).ok()?;
    serde_json::from_reader(file.take(64 * 1024)).ok()
}
fn write_manifest(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let temporary = path.with_extension(format!("{}.part", random_hex()?));
    let contents = serde_json::to_vec_pretty(value)
        .map_err(|_| "Cannot encode embedding installation metadata".to_string())?;
    fs::write(&temporary, contents)
        .map_err(|_| "Cannot save embedding installation metadata".to_string())?;
    replace_file(&temporary, path)
}
fn replace_file(source: &Path, destination: &Path) -> Result<(), String> {
    if destination.exists() {
        fs::remove_file(destination).map_err(|_| "Cannot replace embedding asset".to_string())?;
    }
    fs::rename(source, destination)
        .map_err(|_| "Cannot finish embedding asset installation".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn document_prompts_follow_the_actual_model_contract() {
        let input = document_input(&EmbeddingDocument {
            title: None,
            text: "제주 여행 일정".into(),
        })
        .unwrap();
        assert_eq!(input, "title: none | text: 제주 여행 일정");
        assert!(document_input(&EmbeddingDocument {
            title: None,
            text: " ".into()
        })
        .is_err());
    }
    #[test]
    fn mrl_truncation_is_renormalized_and_rejects_invalid_outputs() {
        let mut raw = vec![0.0; 768];
        raw[0] = 3.0;
        raw[1] = 4.0;
        raw[700] = 10.0;
        let vector = normalize(raw, 128).unwrap();
        assert_eq!(vector.len(), 128);
        assert!((vector[0] - 0.6).abs() < 1e-6);
        assert!((vector[1] - 0.8).abs() < 1e-6);
        assert!(normalize(vec![0.0; 768], 256).is_err());
        assert!(normalize(vec![f32::NAN; 768], 768).is_err());
        assert!(normalize(vec![1.0; 767], 768).is_err());
        assert!(normalize(vec![1.0; 768], 100).is_err());
    }
    #[test]
    fn archive_paths_reject_traversal_streams_and_nested_entries() {
        for path in [
            "../escape",
            "C:\\escape",
            "folder/file.dll",
            "file:stream",
            "/absolute",
            ".",
            "",
        ] {
            assert!(!safe_flat_name(Path::new(path)), "{path}");
        }
        assert!(safe_flat_name(Path::new("llama-server.exe")));
    }
    #[test]
    fn state_survives_an_empty_install_and_reports_bad_imports() {
        let directory = tempfile::tempdir().unwrap();
        let service = EmbeddingRuntime::new(directory.path().to_path_buf()).unwrap();
        assert_eq!(service.status().phase, EmbeddingPhase::NotInstalled);
        assert!(service.import_model(Path::new("relative.gguf")).is_err());
        assert_eq!(service.status().phase, EmbeddingPhase::Error);
        assert!(!service.status().model_ready);
        assert_eq!(service.stop().unwrap().phase, EmbeddingPhase::NotInstalled);
    }
    #[test]
    fn concurrent_operation_is_rejected_without_resetting_cancel() {
        let directory = tempfile::tempdir().unwrap();
        let service = EmbeddingRuntime::new(directory.path().to_path_buf()).unwrap();
        let _guard = lock(&service.inner.operation);
        service.cancel_download();
        assert!(service
            .import_model(Path::new("unused"))
            .unwrap_err()
            .contains("already running"));
        assert!(service.inner.cancel.load(Ordering::Acquire));
    }
    #[test]
    fn stalled_download_reader_cancels_without_waiting_for_network() {
        let (_sender, chunks) = mpsc::sync_channel(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let mut reader = DownloadReader {
            chunks,
            pending: Cursor::new(Vec::new()),
            cancel: Arc::clone(&cancel),
        };
        let worker = thread::spawn(move || reader.read(&mut [0u8; 4]));
        cancel.store(true, Ordering::Release);
        let start = Instant::now();
        assert!(worker.join().unwrap().is_err());
        assert!(start.elapsed() < Duration::from_secs(1));
    }
    #[test]
    #[ignore = "Requires PRISM_TEST_MODEL and PRISM_TEST_RUNTIME pinned local assets"]
    fn real_korean_retrieval_and_owned_process_shutdown() {
        let model = PathBuf::from(std::env::var_os("PRISM_TEST_MODEL").expect("PRISM_TEST_MODEL"));
        let runtime =
            PathBuf::from(std::env::var_os("PRISM_TEST_RUNTIME").expect("PRISM_TEST_RUNTIME"));
        let directory = tempfile::tempdir().unwrap();
        let service = EmbeddingRuntime::new(directory.path().to_path_buf()).unwrap();
        service.import_model(&model).unwrap();
        assert!(!service.status().runtime_ready);
        service.import_runtime(&runtime).unwrap();
        assert_eq!(service.status().phase, EmbeddingPhase::Ready);
        let installed_directory = lock(&service.inner.runtime)
            .as_ref()
            .unwrap()
            .directory
            .clone();
        service.import_runtime(&runtime).unwrap();
        assert_eq!(
            lock(&service.inner.runtime).as_ref().unwrap().directory,
            installed_directory
        );
        let before = Instant::now();
        service.start().unwrap();
        let pid = service.status().pid.unwrap();
        assert_eq!(service.status().phase, EmbeddingPhase::Running);
        // Mutating installed assets during inference must leave Running intact.
        assert!(service.import_model(&model).is_err());
        assert_eq!(service.status().phase, EmbeddingPhase::Running);
        let query = service
            .embed_query("여름휴가 때 제주도에 가는 일정은 어디에 있어?", 768)
            .unwrap();
        let documents = [
            EmbeddingDocument { title: Some("제주도 여름휴가 계획".into()), text: "8월 가족 여행으로 제주도에 갑니다. 항공권은 금요일 아침이고 숙소는 서귀포입니다.".into() },
            EmbeddingDocument { title: Some("회의록".into()), text: "다음 분기 데이터베이스 마이그레이션과 매출 보고서 자동화 작업을 검토했습니다.".into() },
            EmbeddingDocument { title: Some("김치찌개 조리법".into()), text: "돼지고기와 잘 익은 김치를 볶은 뒤 물과 두부를 넣고 끓입니다.".into() },
        ];
        let vectors = service.embed_documents(&documents, 768).unwrap();
        let scores: Vec<f32> = vectors
            .iter()
            .map(|vector| query.iter().zip(vector).map(|(a, b)| a * b).sum())
            .collect();
        assert!(scores[0] > scores[1] && scores[0] > scores[2], "{scores:?}");
        assert_eq!(service.embed_query("제주도 여행", 256).unwrap().len(), 256);
        assert!(service.fingerprint(768).unwrap().contains(MODEL.sha256));
        println!(
            "real model: pid={pid}, elapsed={:?}, dimensions={}, scores={scores:?}",
            before.elapsed(),
            query.len()
        );
        // Dropping the last service handle must close its server socket and reap its child.
        let endpoint = lock(&service.inner.server)
            .as_ref()
            .unwrap()
            .endpoint
            .clone();
        drop(service);
        assert!(local_client(Duration::from_secs(1))
            .unwrap()
            .get(format!("{endpoint}/health"))
            .send()
            .is_err());
        let reopened = EmbeddingRuntime::new(directory.path().to_path_buf()).unwrap();
        assert_eq!(reopened.status().phase, EmbeddingPhase::Ready);
        assert!(reopened.status().pid.is_none());
    }
}
