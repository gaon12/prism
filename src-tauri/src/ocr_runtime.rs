//! App-owned full PaddleOCR-VL pipeline. Installation and recognition block and
//! belong on workers. Status/cancel touch shared state only. Documents travel
//! only to an authenticated, owned loopback child.
use crate::{
    ocr::{self, error, Code, Control, Input, Limits, Page, Result},
    ocr_assets::{self as assets, Catalog, Entry, Kind},
    ocr_package as package,
    runtime_process::OwnedRuntimeProcess,
};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex, OnceLock,
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub phase: String,
    pub target: String,
    pub pipeline: String,
    pub available: bool,
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    pub expanded_bytes: u64,
    pub processed_files: usize,
    pub total_files: usize,
    pub pid: Option<u32>,
    pub last_error: Option<Code>,
}

#[cfg(all(test, target_os = "macos", target_arch = "aarch64"))]
#[path = "../../tools/platform-qa/tests/support/macos_exit.rs"]
mod macos_exit;

#[cfg(all(
    test,
    any(
        windows,
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "macos", target_arch = "aarch64")
    )
))]
mod tests {
    use super::*;
    fn launcher() -> PathBuf {
        #[cfg(windows)]
        {
            std::env::current_exe().unwrap()
        }
        #[cfg(unix)]
        {
            PathBuf::from(std::env::var_os("PRISM_TEST_LAUNCHER").expect(
                "Set PRISM_TEST_LAUNCHER to the QA binary with pre-GUI supervisor dispatch",
            ))
        }
    }
    #[test]
    #[ignore = "Diagnostic using an already materialized, fully authenticated official runtime."]
    fn real_paddle_startup_diagnostic() {
        let root = PathBuf::from(
            std::env::var_os("PRISM_TEST_PADDLE_INSTALLED")
                .expect("Set PRISM_TEST_PADDLE_INSTALLED"),
        );
        let catalog = assets::for_target(assets::target()).unwrap();
        let files = package::load(&root, &catalog).unwrap();
        let app = tempfile::tempdir().unwrap();
        let runtime = Runtime::open(app.path(), &launcher(), |status| {
            eprintln!("paddle diagnostic phase={}", status.phase)
        })
        .unwrap();
        *runtime.inner.installed.lock().unwrap() = Some(Installed {
            directory: root,
            files: Arc::new(files),
        });
        let cancel = AtomicBool::new(false);
        let control = Control {
            cancel: &cancel,
            deadline: Instant::now() + Duration::from_secs(180),
            _lease: ocr::Lease::acquire().unwrap(),
        };
        let value = runtime
            .recognize(
                Input::Image(include_bytes!("../../tests/fixtures/ocr/bilingual.png")),
                &Limits::default(),
                &control,
            )
            .unwrap();
        assert!(value[0].text.contains("74829") && value[0].text.contains("프리즘"));
        runtime.shutdown();
    }
    #[test]
    #[ignore = "Requires the pinned 2+ GiB official artifact directory; runs the actual complete CPU pipeline."]
    fn real_paddle_install_recognition_and_cancellation() {
        let offline = if std::env::var("PRISM_TEST_PADDLE_NETWORK").as_deref() == Ok("1") {
            None
        } else {
            Some(PathBuf::from(
                std::env::var_os("PRISM_TEST_PADDLE_OFFLINE").expect(
                    "Set PRISM_TEST_PADDLE_OFFLINE to the pinned offline artifact directory",
                ),
            ))
        };
        let directory = tempfile::tempdir().unwrap();
        let runtime = Runtime::open(directory.path(), &launcher(), |status| {
            eprintln!(
                "paddle {} bytes={} files={}",
                status.phase, status.downloaded_bytes, status.processed_files
            );
        })
        .unwrap();
        assert!(!runtime.status().available);
        let installed = runtime.install(offline.as_deref()).unwrap();
        assert!(installed.available);
        assert_eq!(installed.processed_files, installed.total_files);
        assert_eq!(installed.downloaded_bytes, installed.total_bytes);
        // Reopening uses the authenticated inventory; no Python from PATH or
        // user packages participates in this production runtime path.
        runtime.shutdown();
        let runtime = Runtime::open(directory.path(), &launcher(), |_| {}).unwrap();
        let installed = runtime.inner.installed.lock().unwrap().clone().unwrap();
        let injected = installed.directory.join("python/foreign_import.py");
        fs::write(&injected, b"raise RuntimeError('must never load')").unwrap();
        let cancel = AtomicBool::new(false);
        let mut control = Control {
            cancel: &cancel,
            deadline: Instant::now() + Duration::from_secs(180),
            _lease: ocr::Lease::acquire().unwrap(),
        };
        let image = include_bytes!("../../tests/fixtures/ocr/bilingual.png");
        let limits = Limits::default();
        assert_eq!(
            runtime
                .recognize(Input::Image(image), &limits, &control)
                .unwrap_err()
                .code,
            Code::Failed
        );
        assert!(runtime.status().pid.is_none());
        fs::remove_file(injected).unwrap();
        control.deadline = Instant::now() + Duration::from_secs(180);
        let recognized = runtime
            .recognize(Input::Image(image), &limits, &control)
            .unwrap();
        assert_eq!(recognized.len(), 1);
        assert_eq!(recognized[0].number, 1);
        assert!(recognized[0].text.contains("프리즘") && recognized[0].text.contains("74829"));
        let pid = runtime.status().pid.unwrap();
        // Retain the live child's exact kernel identity across cancellation;
        // checking a PID after the child exits could observe a reused PID.
        #[cfg(windows)]
        let process = {
            use std::os::windows::io::{FromRawHandle, OwnedHandle};
            use windows_sys::Win32::System::Threading::OpenProcess;
            let handle = unsafe { OpenProcess(0x00100000, 0, pid) };
            assert!(!handle.is_null());
            unsafe { OwnedHandle::from_raw_handle(handle) }
        };
        #[cfg(target_os = "linux")]
        let process = {
            use std::os::fd::{FromRawFd, OwnedFd};
            let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
            assert!(
                fd >= 0,
                "retain the live owned worker identity with pidfd_open"
            );
            unsafe { OwnedFd::from_raw_fd(fd as i32) }
        };
        #[cfg(target_os = "macos")]
        let process = macos_exit::ExitProbe::new(pid)
            .expect("retain the live owned worker's kernel exit notification");
        eprintln!("paddle actual image passed");
        control.deadline = Instant::now() + Duration::from_secs(180);
        let recognized = runtime
            .recognize(
                Input::Pdf {
                    bytes: include_bytes!("../../tests/fixtures/ocr/mixed.pdf"),
                    pages: &[2],
                },
                &limits,
                &control,
            )
            .unwrap();
        assert_eq!(recognized.len(), 1);
        assert_eq!(recognized[0].number, 2);
        assert!(recognized[0].text.contains("프리즘") && recognized[0].text.contains("74829"));
        assert_eq!(
            runtime.status().pid,
            Some(pid),
            "reuse the same owned model process"
        );
        eprintln!("paddle actual PDF passed and process reused");
        let started = Instant::now();
        control.deadline = Instant::now() + Duration::from_secs(180);
        thread::scope(|scope| {
            scope.spawn(|| {
                thread::sleep(Duration::from_millis(150));
                cancel.store(true, Ordering::Release);
            });
            assert_eq!(
                runtime
                    .recognize(Input::Image(image), &limits, &control)
                    .unwrap_err()
                    .code,
                Code::Cancelled
            );
        });
        assert!(started.elapsed() < Duration::from_secs(15));
        assert!(runtime.status().pid.is_none());
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::System::Threading::WaitForSingleObject;
            assert_eq!(
                unsafe { WaitForSingleObject(process.as_raw_handle(), 5000) },
                0,
                "cancel must terminate the actual owned process"
            );
        }
        #[cfg(target_os = "linux")]
        {
            use std::os::fd::AsRawFd;
            let mut ready = libc::pollfd {
                fd: process.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            assert_eq!(unsafe { libc::poll(&mut ready, 1, 5000) }, 1);
            assert_ne!(
                ready.revents & libc::POLLIN,
                0,
                "cancel must terminate the actual owned process"
            );
        }
        #[cfg(target_os = "macos")]
        process
            .wait(Duration::from_secs(5))
            .expect("cancel must terminate the actual owned process");
        eprintln!("paddle actual cancellation and process exit passed");
        runtime.shutdown();
    }
}
#[derive(Clone)]
struct Installed {
    directory: PathBuf,
    files: Arc<Vec<Entry>>,
}
struct Server {
    child: OwnedRuntimeProcess,
    endpoint: String,
    token: String,
    last_use: Instant,
}
struct Inner {
    root: PathBuf,
    launcher: PathBuf,
    catalog: Option<Catalog>,
    installed: Mutex<Option<Installed>>,
    server: Mutex<Option<Server>>,
    status: Mutex<Status>,
    installing: AtomicBool,
    cancel_install: AtomicBool,
    cancel_worker: AtomicBool,
    shutdown: AtomicBool,
    emit: Box<dyn Fn(Status) + Send + Sync>,
    last_emit: Mutex<Instant>,
}
#[derive(Clone)]
pub struct Runtime {
    inner: Arc<Inner>,
}
static RUNTIME: OnceLock<Runtime> = OnceLock::new();
pub fn initialize(
    directory: &Path,
    launcher: &Path,
    emit: impl Fn(Status) + Send + Sync + 'static,
) -> Result<Runtime> {
    let runtime = Runtime::open(directory, launcher, emit)?;
    RUNTIME
        .set(runtime.clone())
        .map_err(|_| error(Code::Busy))?;
    Ok(runtime)
}
pub fn global() -> Option<&'static Runtime> {
    RUNTIME.get()
}
fn lock_error<T>(_: T) -> ocr::Error {
    error(Code::Failed)
}
fn random() -> Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(lock_error)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}
fn local_client(timeout: Duration) -> Result<Client> {
    Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_millis(500))
        .timeout(timeout)
        .build()
        .map_err(lock_error)
}
impl Runtime {
    pub fn open(
        directory: &Path,
        launcher: &Path,
        emit: impl Fn(Status) + Send + Sync + 'static,
    ) -> Result<Self> {
        let root = directory.join("ocr");
        fs::create_dir_all(&root).map_err(lock_error)?;
        let root = crate::path_identity::canonical(&root).map_err(lock_error)?;
        let catalog = match assets::for_target(assets::target()) {
            Ok(catalog) => Some(catalog),
            Err(cause) if cause.code == Code::Unavailable => None,
            Err(cause) => return Err(cause),
        };
        let installed = catalog.as_ref().and_then(|spec| {
            let directory = package::installed(&root, spec);
            package::load(&directory, spec).ok().map(|files| Installed {
                directory,
                files: Arc::new(files),
            })
        });
        let available = installed.is_some();
        let status = Status {
            phase: if catalog.is_none() {
                "unsupported"
            } else if available {
                "ready"
            } else {
                "notInstalled"
            }
            .into(),
            target: assets::target().into(),
            pipeline: assets::PIPELINE.into(),
            available,
            downloaded_bytes: 0,
            total_bytes: catalog.as_ref().map_or(0, Catalog::total_bytes),
            expanded_bytes: catalog.as_ref().map_or(0, |c| c.expanded_bytes),
            processed_files: 0,
            total_files: catalog.as_ref().map_or(0, |c| c.file_count),
            pid: None,
            last_error: None,
        };
        let runtime = Self {
            inner: Arc::new(Inner {
                root,
                launcher: launcher.into(),
                catalog,
                installed: Mutex::new(installed),
                server: Mutex::new(None),
                status: Mutex::new(status),
                installing: AtomicBool::new(false),
                cancel_install: AtomicBool::new(false),
                cancel_worker: AtomicBool::new(false),
                shutdown: AtomicBool::new(false),
                emit: Box::new(emit),
                last_emit: Mutex::new(Instant::now()),
            }),
        };
        let weak = Arc::downgrade(&runtime.inner);
        thread::Builder::new()
            .name("prism-ocr-idle".into())
            .spawn(move || loop {
                thread::sleep(Duration::from_secs(15));
                let Some(inner) = weak.upgrade() else {
                    break;
                };
                if inner.shutdown.load(Ordering::Acquire) {
                    break;
                }
                if let Ok(mut server) = inner.server.try_lock() {
                    if server
                        .as_ref()
                        .is_some_and(|s| s.last_use.elapsed() >= Duration::from_secs(120))
                    {
                        server.take();
                        let runtime = Runtime {
                            inner: inner.clone(),
                        };
                        runtime.update(true, |s| {
                            s.phase = "ready".into();
                            s.pid = None;
                        });
                    }
                };
            })
            .map_err(lock_error)?;
        Ok(runtime)
    }
    pub fn status(&self) -> Status {
        self.inner
            .status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    pub fn cache_tag(&self) -> String {
        let available = self
            .inner
            .installed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some();
        format!(
            "{}-{available}",
            self.inner
                .catalog
                .as_ref()
                .map(|catalog| &catalog.inventory_sha256[..16])
                .unwrap_or("unsupported")
        )
    }
    fn update(&self, force: bool, f: impl FnOnce(&mut Status)) {
        let status = {
            let mut status = self.inner.status.lock().unwrap_or_else(|e| e.into_inner());
            f(&mut status);
            status.clone()
        };
        let mut last = self
            .inner
            .last_emit
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if force || last.elapsed() >= Duration::from_millis(150) {
            *last = Instant::now();
            drop(last);
            (self.inner.emit)(status);
        }
    }
    fn install_check(&self) -> Result<()> {
        if self.inner.shutdown.load(Ordering::Acquire)
            || self.inner.cancel_install.load(Ordering::Acquire)
        {
            Err(error(Code::Cancelled))
        } else {
            Ok(())
        }
    }
    async fn interruptible<T>(&self, future: impl std::future::Future<Output = T>) -> Result<T> {
        let mut pending = Box::pin(future);
        loop {
            self.install_check()?;
            if let Ok(value) = tokio::time::timeout(Duration::from_millis(50), &mut pending).await {
                return Ok(value);
            }
        }
    }
    pub fn cancel_install(&self) {
        self.inner.cancel_install.store(true, Ordering::Release);
    }
    pub fn shutdown(&self) {
        self.inner.shutdown.store(true, Ordering::Release);
        self.cancel_install();
        self.inner.cancel_worker.store(true, Ordering::Release);
        if let Ok(mut server) = self.inner.server.try_lock() {
            server.take();
        }
    }
    /// `offline` is a directory of the same pinned artifacts, not an executable
    /// or a user-provided runtime manifest. No installer from that directory runs.
    pub fn install(&self, offline: Option<&Path>) -> Result<Status> {
        let catalog = self
            .inner
            .catalog
            .as_ref()
            .ok_or_else(|| error(Code::Unavailable))?;
        if self.inner.installing.swap(true, Ordering::AcqRel) {
            return Err(error(Code::Busy));
        }
        struct Release<'a>(&'a AtomicBool);
        impl Drop for Release<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Release);
            }
        }
        let _release = Release(&self.inner.installing);
        if self.inner.installed.lock().map_err(lock_error)?.is_some() {
            return Ok(self.status());
        }
        self.inner.cancel_install.store(false, Ordering::Release);
        self.install_check()?;
        let offline = offline
            .map(|path| crate::path_identity::canonical(path).map_err(lock_error))
            .transpose()?;
        let stage = self.inner.root.join(format!("stage-{}", random()?));
        fs::create_dir(&stage).map_err(lock_error)?;
        self.update(true, |s| {
            s.phase = "installing".into();
            s.downloaded_bytes = 0;
            s.processed_files = 0;
            s.last_error = None;
        });
        let result = (|| {
            let executor = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(lock_error)?;
            let client = reqwest::Client::builder()
                .https_only(true)
                .connect_timeout(Duration::from_secs(15))
                .read_timeout(Duration::from_secs(20))
                .timeout(Duration::from_secs(1800))
                .build()
                .map_err(lock_error)?;
            let mut files = Vec::new();
            let mut completed = 0u64;
            for asset in &catalog.assets {
                self.install_check()?;
                let artifact = if asset.kind == Kind::Model {
                    stage.join(assets::relative(&asset.path)?)
                } else {
                    stage.join(format!("{}.download", asset.sha256))
                };
                fs::create_dir_all(artifact.parent().ok_or_else(|| error(Code::Failed))?)
                    .map_err(lock_error)?;
                let mut output = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&artifact)
                    .map_err(lock_error)?;
                let mut bytes = 0u64;
                let mut hash = sha2::Sha256::new();
                use sha2::Digest;
                let mut consume = |chunk: &[u8]| -> Result<()> {
                    self.install_check()?;
                    bytes += chunk.len() as u64;
                    if bytes > asset.bytes {
                        return Err(error(Code::LimitExceeded));
                    }
                    output.write_all(chunk).map_err(lock_error)?;
                    hash.update(chunk);
                    self.update(false, |s| s.downloaded_bytes = completed + bytes);
                    Ok(())
                };
                if let Some(offline) = &offline {
                    let mut source =
                        package::regular(&offline.join(assets::relative(&asset.path)?))?;
                    let mut buffer = [0; 64 << 10];
                    loop {
                        self.install_check()?;
                        let n = source.read(&mut buffer).map_err(lock_error)?;
                        if n == 0 {
                            break;
                        }
                        consume(&buffer[..n])?;
                    }
                } else {
                    executor.block_on(async {
                        let mut response = self
                            .interruptible(client.get(&asset.url).send())
                            .await?
                            .map_err(lock_error)?;
                        if !response.status().is_success()
                            || response.content_length().is_some_and(|n| n != asset.bytes)
                        {
                            return Err(error(Code::Failed));
                        }
                        while let Some(chunk) = self
                            .interruptible(response.chunk())
                            .await?
                            .map_err(lock_error)?
                        {
                            consume(&chunk)?;
                        }
                        Ok(())
                    })?;
                }
                if bytes != asset.bytes || format!("{:x}", hash.finalize()) != asset.sha256 {
                    return Err(error(Code::Failed));
                }
                output.sync_all().map_err(lock_error)?;
                drop(output);
                completed += bytes;
                if asset.kind == Kind::Model {
                    files.push(Entry {
                        path: asset.path.clone(),
                        bytes,
                        sha256: asset.sha256.clone(),
                    });
                } else {
                    package::unpack(
                        &artifact,
                        asset.kind,
                        catalog,
                        &stage,
                        &mut files,
                        &|| self.install_check(),
                        &|count| self.update(false, |status| status.processed_files = count),
                    )?;
                    fs::remove_file(&artifact).map_err(lock_error)?;
                }
                self.update(false, |s| {
                    s.downloaded_bytes = completed;
                    s.processed_files = files.len();
                });
            }
            package::write(
                &stage,
                &catalog.bootstrap_path,
                catalog.bootstrap.as_bytes(),
                &mut files,
            )?;
            package::write(&stage, "worker.py", assets::WORKER, &mut files)?;
            catalog.validate_inventory(&files)?;
            self.install_check()?;
            let inventory = serde_json::to_vec(&files).map_err(lock_error)?;
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(stage.join("inventory.json"))
                .map_err(lock_error)?;
            file.write_all(&inventory).map_err(lock_error)?;
            file.sync_all().map_err(lock_error)?;
            drop(file);
            let directory = package::installed(&self.inner.root, catalog);
            if fs::symlink_metadata(&directory).is_ok() {
                return Err(error(Code::Failed));
            }
            fs::rename(&stage, &directory).map_err(lock_error)?;
            *self.inner.installed.lock().map_err(lock_error)? = Some(Installed {
                directory,
                files: Arc::new(files),
            });
            self.update(true, |s| {
                s.phase = "ready".into();
                s.available = true;
                s.downloaded_bytes = s.total_bytes;
                s.processed_files = s.total_files;
            });
            Ok(self.status())
        })();
        if let Err(cause) = &result {
            package::cleanup(&self.inner.root, &stage);
            self.update(true, |s| {
                s.phase = if cause.code == Code::Cancelled {
                    "cancelled"
                } else {
                    "error"
                }
                .into();
                s.last_error = Some(cause.code.clone());
            });
        }
        result
    }
    fn check(&self, control: &Control<'_>) -> Result<()> {
        control.check()?;
        if self.inner.shutdown.load(Ordering::Acquire)
            || self.inner.cancel_worker.load(Ordering::Acquire)
        {
            Err(error(Code::Cancelled))
        } else {
            Ok(())
        }
    }
    fn start(&self, control: &Control<'_>) -> Result<Server> {
        self.check(control)?;
        let catalog = self
            .inner
            .catalog
            .as_ref()
            .ok_or_else(|| error(Code::Unavailable))?;
        let installed = self
            .inner
            .installed
            .lock()
            .map_err(lock_error)?
            .clone()
            .ok_or_else(|| error(Code::Unavailable))?;
        self.update(true, |s| {
            s.phase = "starting".into();
            s.last_error = None;
        });
        #[cfg(test)]
        let verify_started = Instant::now();
        package::verify(&installed.directory, catalog, &installed.files, &|| {
            self.check(control)
        })?;
        #[cfg(test)]
        eprintln!(
            "paddle verify elapsed_ms={} remaining_ms={}",
            verify_started.elapsed().as_millis(),
            control
                .deadline
                .saturating_duration_since(Instant::now())
                .as_millis()
        );
        for _ in 0..3 {
            self.check(control)?;
            let reservation = TcpListener::bind(("127.0.0.1", 0)).map_err(lock_error)?;
            let port = reservation.local_addr().map_err(lock_error)?.port();
            let endpoint = format!("http://127.0.0.1:{port}");
            let token = format!("{}{}", random()?, random()?);
            let mut command = Command::new(installed.directory.join(&catalog.executable));
            command.current_dir(&installed.directory).args(["-I", "-B"]);
            if catalog.bootstrap_entrypoint {
                command
                    .arg("-S")
                    .arg(installed.directory.join(&catalog.bootstrap_path));
            } else {
                command.arg(installed.directory.join("worker.py"));
            }
            command
                .arg("--root")
                .arg(&installed.directory)
                .arg("--port")
                .arg(port.to_string())
                .env("PRISM_OCR_TOKEN", &token)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            #[cfg(test)]
            if let Some(path) = std::env::var_os("PRISM_TEST_PADDLE_STDERR") {
                command.stderr(
                    OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(path)
                        .map_err(lock_error)?,
                );
            }
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                command.creation_flags(0x08000000);
            }
            drop(reservation);
            let child =
                OwnedRuntimeProcess::spawn(command, &self.inner.launcher).map_err(lock_error)?;
            let mut server = Server {
                child,
                endpoint,
                token,
                last_use: Instant::now(),
            };
            let deadline = Instant::now() + Duration::from_secs(90);
            let client = local_client(Duration::from_millis(500))?;
            let nonce = random()?;
            let expected = assets::digest(format!("{}{nonce}", server.token).as_bytes());
            while Instant::now() < deadline {
                self.check(control)?;
                if !matches!(server.child.try_wait(), Ok(None)) {
                    break;
                }
                if let Ok(response) = client
                    .get(format!("{}/v1/health?nonce={nonce}", server.endpoint))
                    .send()
                {
                    let mut data = Vec::new();
                    if response.status().is_success()
                        && response.take(4097).read_to_end(&mut data).is_ok()
                        && data.len() <= 4096
                    {
                        if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&data) {
                            if value["pipeline"] == assets::PIPELINE && value["proof"] == expected {
                                #[cfg(test)]
                                eprintln!("paddle owned health proof matched");
                                self.update(true, |s| {
                                    s.phase = "running".into();
                                    s.pid = Some(server.child.id());
                                });
                                return Ok(server);
                            }
                        }
                    }
                }
                thread::sleep(Duration::from_millis(50));
            }
        }
        Err(error(Code::Failed))
    }
    pub(crate) fn recognize(
        &self,
        input: Input<'_>,
        limits: &Limits,
        control: &Control<'_>,
    ) -> Result<Vec<Page>> {
        self.check(control)?;
        if self.inner.installing.load(Ordering::Acquire) {
            return Err(error(Code::Busy));
        }
        let mut current = self
            .inner
            .server
            .try_lock()
            .map_err(|_| error(Code::Busy))?;
        self.inner.cancel_worker.store(false, Ordering::Release);
        if current
            .as_mut()
            .is_some_and(|server| !matches!(server.child.try_wait(), Ok(None)))
        {
            current.take();
        }
        if current.is_none() {
            match self.start(control) {
                Ok(server) => *current = Some(server),
                Err(cause) => {
                    self.update(true, |s| {
                        s.phase = "error".into();
                        s.pid = None;
                        s.last_error = Some(cause.code.clone());
                    });
                    return Err(cause);
                }
            }
        }
        let server = current.as_mut().ok_or_else(|| error(Code::Unavailable))?;
        let (bytes, kind, pages) = match input {
            Input::Image(bytes) => (bytes, "image", vec![1]),
            Input::Pdf { bytes, pages } => (bytes, "pdf", pages.to_vec()),
        };
        let options = serde_json::json!({"kind":kind,"pages":pages,"maxSourcePixels":limits.max_source_pixels.min(64_000_000),"maxPixels":limits.max_pixels.min(8_000_000),"maxDimension":limits.max_dimension.min(4096),"maxTextBytes":limits.max_text_bytes.min(32<<20),"durationMs":control.deadline.saturating_duration_since(Instant::now()).as_millis().min(3_600_000)});
        let request = local_client(control.deadline.saturating_duration_since(Instant::now()))?
            .post(format!("{}/v1/recognize", server.endpoint))
            .bearer_auth(&server.token)
            .header("X-Prism-OCR", options.to_string())
            .header("Content-Type", "application/octet-stream")
            .body(bytes.to_vec());
        let max_reply = (limits.max_text_bytes as u64).min(32 << 20) * 6 + (64 << 10);
        let result = thread::scope(|scope| {
            let (send, receive) = mpsc::sync_channel(1);
            scope.spawn(move || {
                let answer = (|| {
                    let response = request.send().map_err(|e| {
                        error(if e.is_timeout() {
                            Code::TimedOut
                        } else {
                            Code::Failed
                        })
                    })?;
                    let mut bytes = Vec::new();
                    response
                        .take(max_reply + 1)
                        .read_to_end(&mut bytes)
                        .map_err(lock_error)?;
                    if bytes.len() as u64 > max_reply {
                        return Err(error(Code::LimitExceeded));
                    }
                    Ok(bytes)
                })();
                let _ = send.send(answer);
            });
            loop {
                if let Err(error) = self.check(control) {
                    current.take();
                    break Err(error);
                }
                match receive.recv_timeout(Duration::from_millis(50)) {
                    Ok(answer) => break answer,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(_) => break Err(error(Code::Failed)),
                }
            }
        });
        let result = result.and_then(|bytes| {
            #[derive(Deserialize)]
            struct WirePage {
                number: u32,
                text: String,
            }
            #[derive(Deserialize)]
            struct Reply {
                engine: Option<String>,
                pages: Option<Vec<WirePage>>,
                error: Option<Code>,
            }
            let reply: Reply = serde_json::from_slice(&bytes).map_err(lock_error)?;
            if let Some(code) = reply.error {
                return Err(error(code));
            }
            let output = reply.pages.ok_or_else(|| error(Code::Failed))?;
            if reply.engine.as_deref() != Some("paddleVl16")
                || output.len() != pages.len()
                || output.iter().zip(&pages).any(|(a, b)| a.number != *b)
                || output.iter().map(|p| p.text.len()).sum::<usize>() > limits.max_text_bytes
            {
                return Err(error(Code::Failed));
            }
            Ok(output
                .into_iter()
                .map(|page| Page {
                    number: page.number,
                    text: page.text,
                })
                .collect())
        });
        if let Some(server) = current.as_mut() {
            server.last_use = Instant::now();
        }
        if result.as_ref().is_err_and(|cause| {
            matches!(cause.code, Code::TimedOut | Code::Cancelled | Code::Failed)
        }) {
            current.take();
        }
        self.update(true, |s| {
            s.pid = current.as_ref().map(|server| server.child.id());
            s.phase = if s.pid.is_some() { "running" } else { "ready" }.into();
            s.last_error = result.as_ref().err().map(|cause| cause.code.clone());
        });
        result
    }
}
