//! Own one verified local renderer and one bounded request. The caller still
//! authenticates source bytes and rechecks permissions/fingerprint afterwards.
use crate::office_assets::{self as assets, error, Catalog, Code, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

const WORK_OWNER: &[u8] = b"Prism Office preview worker v1\n";
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub phase: &'static str,
    pub files: usize,
    pub total_files: usize,
}
struct Inner {
    parent: PathBuf,
    catalog: Catalog,
    gate: Mutex<()>,
}
#[derive(Clone)]
pub struct Runtime(Arc<Inner>);
impl Runtime {
    pub fn open(parent: PathBuf) -> Result<Self> {
        if !parent.is_absolute() {
            return Err(error(Code::Input));
        }
        crate::path_identity::comparison(&parent).map_err(|_| error(Code::Integrity))?;
        Ok(Self(Arc::new(Inner {
            parent,
            catalog: assets::catalog()?,
            gate: Mutex::new(()),
        })))
    }
    pub fn fingerprint(&self) -> String {
        let worker = if cfg!(target_os = "linux") {
            assets::digest(&[assets::WORKER, assets::UNIX_OWNER].concat())
        } else {
            assets::digest(assets::WORKER)
        };
        format!("{}:{}", self.0.catalog.inventory_sha256, worker)
    }
    /// Presence only, not a verification result. Every execution still checks
    /// the full sealed inventory, and a damaged installed tree is an error.
    pub fn installed(&self) -> Result<bool> {
        if !supported() {
            return Ok(false);
        }
        self.0
            .parent
            .join(self.0.catalog.directory())
            .try_exists()
            .map_err(|_| error(Code::Integrity))
    }
    pub fn install_archive(
        &self,
        path: &Path,
        cancel: &AtomicBool,
        deadline: Instant,
        progress: &dyn Fn(Progress),
    ) -> Result<()> {
        let _lease = self.0.gate.try_lock().map_err(|_| error(Code::Busy))?;
        let check = || check(cancel, deadline);
        progress(Progress {
            phase: "installing",
            files: 0,
            total_files: self.0.catalog.file_count,
        });
        assets::install_archive(&self.0.parent, path, &self.0.catalog, &check, &|n| {
            if n % 1000 == 0 || n == self.0.catalog.file_count {
                progress(Progress {
                    phase: "installing",
                    files: n,
                    total_files: self.0.catalog.file_count,
                });
            }
        })?;
        progress(Progress {
            phase: "ready",
            files: self.0.catalog.file_count,
            total_files: self.0.catalog.file_count,
        });
        Ok(())
    }
    pub fn render_pdf(
        &self,
        extension: &str,
        input: &[u8],
        cancel: &AtomicBool,
        deadline: Instant,
        progress: &dyn Fn(Progress),
    ) -> Result<Vec<u8>> {
        if !supported() {
            return Err(error(Code::Unsupported));
        }
        check(cancel, deadline)?;
        if !matches!(extension, "docx" | "pptx" | "xlsx") {
            return Err(error(Code::Unsupported));
        }
        if input.is_empty() || input.len() > assets::MAX_DOCUMENT {
            return Err(error(Code::Limit));
        }
        let _lease = self.0.gate.try_lock().map_err(|_| error(Code::Busy))?;
        let root = self.0.parent.join(self.0.catalog.directory());
        if !root.exists() {
            return Err(error(Code::Unavailable));
        }
        progress(Progress {
            phase: "verifying",
            files: 0,
            total_files: self.0.catalog.file_count,
        });
        assets::verify(&root, &self.0.catalog, &|| check(cancel, deadline), &|n| {
            if n % 1000 == 0 || n == self.0.catalog.file_count {
                progress(Progress {
                    phase: "verifying",
                    files: n,
                    total_files: self.0.catalog.file_count,
                });
            }
        })?;
        let work = Work::new(&self.0.parent.join("work"))?;
        let runtime = crate::path_identity::normal(&root);
        let directory = crate::path_identity::normal(&work.path);
        let interpreter = runtime.join(assets::relative(&self.0.catalog.executable)?);
        let mut command = Command::new(interpreter);
        command
            .arg(if cfg!(target_os = "linux") {
                "-P"
            } else {
                "-I"
            })
            .arg("-S")
            .arg("-B")
            .arg(directory.join("worker.py"))
            .arg(&runtime)
            .arg(&directory)
            .current_dir(&directory)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // No user PATH, PYTHONPATH, add-on or Python startup configuration is
        // inherited. Runtime libraries come from the verified private engine.
        let mut paths = vec![
            runtime.join("program"),
            runtime.join("program/python-core-3.13.17/bin"),
        ];
        if let Some(windows) = std::env::var_os("SystemRoot") {
            command.env("SystemRoot", &windows).env("WINDIR", &windows);
            paths.push(PathBuf::from(windows).join("System32"));
        }
        #[cfg(target_os = "linux")]
        {
            let home = runtime.join(assets::relative(
                self.0
                    .catalog
                    .python_home
                    .as_deref()
                    .ok_or_else(|| error(Code::Integrity))?,
            )?);
            command
                .env("PYTHONHOME", &home)
                .env(
                    "PYTHONPATH",
                    std::env::join_paths([home.join("lib"), home.join("lib/lib-dynload")])
                        .map_err(|_| error(Code::Input))?,
                )
                .env(
                    "LD_LIBRARY_PATH",
                    std::env::join_paths([
                        runtime.join("program/prism-libs/nss"),
                        runtime.join("program/prism-libs"),
                        runtime.join("program"),
                    ])
                    .map_err(|_| error(Code::Input))?,
                )
                .env("HOME", directory.join("home"))
                .env("XDG_CONFIG_HOME", directory.join("config"))
                .env("XDG_DATA_HOME", directory.join("data"))
                .env("XDG_CACHE_HOME", directory.join("cache"))
                .env("TMPDIR", &directory);
            paths.extend([PathBuf::from("/usr/bin"), PathBuf::from("/bin")]);
        }
        command
            .env(
                "PATH",
                std::env::join_paths(paths).map_err(|_| error(Code::Input))?,
            )
            .env("TEMP", &directory)
            .env("TMP", &directory);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        progress(Progress {
            phase: "starting",
            files: 0,
            total_files: 0,
        });
        let header = serde_json::to_vec(
            &serde_json::json!({"version":1,"extension":extension,"bytes":input.len()}),
        )
        .map_err(|_| error(Code::Protocol))?;
        let result = transact(command, header, input, cancel, deadline, &|| {
            progress(Progress {
                phase: "rendering",
                files: 0,
                total_files: 0,
            });
        })?;
        check(cancel, deadline)?;
        progress(Progress {
            phase: "ready",
            files: 0,
            total_files: 0,
        });
        Ok(result)
    }
}
pub fn supported() -> bool {
    cfg!(all(
        any(windows, target_os = "linux"),
        target_arch = "x86_64"
    ))
}
fn check(cancel: &AtomicBool, deadline: Instant) -> Result<()> {
    if cancel.load(Ordering::Acquire) {
        Err(error(Code::Cancelled))
    } else if Instant::now() >= deadline {
        Err(error(Code::TimedOut))
    } else {
        Ok(())
    }
}

struct Work {
    parent: PathBuf,
    path: PathBuf,
}
impl Work {
    fn new(parent: &Path) -> Result<Self> {
        assets::mkdir(parent)?;
        let parent = crate::path_identity::canonical(parent).map_err(|_| error(Code::Integrity))?;
        let mut nonce = [0; 16];
        getrandom::fill(&mut nonce).map_err(|_| error(Code::Failed))?;
        // LibreOffice appends deep extension-registry paths to its profile.
        // Retain 128 bits of randomness without spending MAX_PATH's budget on
        // a redundant 64-character digest and long request prefix.
        let path = parent.join(format!("r-{}", &assets::digest(&nonce)[..32]));
        fs::create_dir(&path).map_err(|_| error(Code::Failed))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
                .map_err(|_| error(Code::Failed))?;
        }
        let work = Self { parent, path };
        for (name, bytes) in [("owner.txt", WORK_OWNER), ("worker.py", assets::WORKER)] {
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(work.path.join(name))
                .and_then(|mut file| file.write_all(bytes))
                .map_err(|_| error(Code::Failed))?;
        }
        #[cfg(target_os = "linux")]
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(work.path.join("unix_owner.py"))
            .and_then(|mut file| file.write_all(assets::UNIX_OWNER))
            .map_err(|_| error(Code::Failed))?;
        Ok(work)
    }
}
impl Drop for Work {
    fn drop(&mut self) {
        // This freshly created directory contains app code/private engine
        // temporaries, never a user-selected original. Confirm its resolved
        // parent and exact ownership marker before any recursive cleanup.
        let safe = || -> Result<bool> {
            let path =
                crate::path_identity::canonical(&self.path).map_err(|_| error(Code::Integrity))?;
            let mut marker = Vec::new();
            assets::regular(&path.join("owner.txt"))?
                .take(128)
                .read_to_end(&mut marker)
                .map_err(|_| error(Code::Integrity))?;
            Ok(path.parent() == Some(self.parent.as_path()) && marker == WORK_OWNER)
        };
        if safe().unwrap_or(false) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}
struct OwnedChild(Child);
impl OwnedChild {
    fn stop(&mut self) {
        #[cfg(target_os = "linux")]
        {
            // Closing the parent's writer lets the worker close its supervisor
            // pipe and reap the private engine group before this Child returns.
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline {
                if matches!(self.0.try_wait(), Ok(Some(_))) {
                    return;
                }
                thread::sleep(Duration::from_millis(10));
            }
        }
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        self.stop();
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Response {
    version: u8,
    ok: bool,
    pdf_bytes: Option<usize>,
    code: Option<Code>,
    interactions: Option<u32>,
    elapsed_ms: Option<u64>,
}
fn response(reader: impl Read) -> Result<Vec<u8>> {
    let mut reader = BufReader::new(reader);
    let mut header = Vec::new();
    reader
        .by_ref()
        .take(4097)
        .read_until(b'\n', &mut header)
        .map_err(|_| error(Code::Protocol))?;
    if header.len() > 4096 || !header.ends_with(b"\n") {
        return Err(error(Code::Protocol));
    }
    let value: Response = serde_json::from_slice(&header).map_err(|_| error(Code::Protocol))?;
    if value.version != 1 {
        return Err(error(Code::Protocol));
    }
    if !value.ok {
        if value.pdf_bytes.is_some() {
            return Err(error(Code::Protocol));
        }
        return Err(error(value.code.ok_or_else(|| error(Code::Protocol))?));
    }
    let size = value.pdf_bytes.ok_or_else(|| error(Code::Protocol))?;
    if size == 0
        || size > assets::MAX_DOCUMENT
        || value.code.is_some()
        || value.interactions.is_none()
        || value.elapsed_ms.is_none()
    {
        return Err(error(Code::Protocol));
    }
    let mut bytes = Vec::with_capacity(size);
    reader
        .take((size + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| error(Code::Protocol))?;
    if bytes.len() != size || !bytes.starts_with(b"%PDF-") {
        return Err(error(Code::Protocol));
    }
    Ok(bytes)
}
fn transact(
    mut command: Command,
    mut header: Vec<u8>,
    input: &[u8],
    cancel: &AtomicBool,
    deadline: Instant,
    rendering: &dyn Fn(),
) -> Result<Vec<u8>> {
    check(cancel, deadline)?;
    header.push(b'\n');
    let written = AtomicBool::new(false);
    let failed = AtomicBool::new(false);
    thread::scope(|scope| {
        // Keep the child guard inside the scope closure: on a panic it is
        // dropped before scope waits for pipe threads, avoiding a join deadlock.
        let mut child = OwnedChild(command.spawn().map_err(|_| error(Code::Unavailable))?);
        let mut stdin = child.0.stdin.take().ok_or_else(|| error(Code::Failed))?;
        let stdout = child.0.stdout.take().ok_or_else(|| error(Code::Failed))?;
        let stderr = child.0.stderr.take().ok_or_else(|| error(Code::Failed))?;
        let (release, released) = mpsc::channel::<()>();
        let wrote = &written;
        let fault = &failed;
        let writer = scope.spawn(move || -> Result<()> {
            if stdin
                .write_all(&header)
                .and_then(|()| stdin.write_all(input))
                .and_then(|()| stdin.flush())
                .is_err()
            {
                fault.store(true, Ordering::Release);
                return Err(error(Code::Protocol));
            }
            wrote.store(true, Ordering::Release);
            // Hold the only parent writer until actual completion/cancellation.
            // Closing it after the body would prematurely trigger the lifeline.
            let _ = released.recv();
            drop(stdin);
            Ok(())
        });
        let reader = scope.spawn(move || {
            let result = response(stdout);
            if result.is_err() {
                fault.store(true, Ordering::Release);
            }
            result
        });
        let errors = scope.spawn(move || -> Result<bool> {
            let mut bytes = Vec::new();
            stderr
                .take(4097)
                .read_to_end(&mut bytes)
                .map_err(|_| error(Code::Failed))?;
            if bytes.len() > 4096 {
                fault.store(true, Ordering::Release);
            }
            Ok(!bytes.is_empty())
        });
        let mut announced = false;
        let mut primary = None;
        let mut status = None;
        loop {
            if let Err(error) = check(cancel, deadline) {
                primary = Some(error);
                break;
            }
            if failed.load(Ordering::Acquire) {
                break;
            }
            if written.load(Ordering::Acquire) && !announced {
                rendering();
                announced = true;
            }
            match child.0.try_wait() {
                Ok(Some(exit)) => {
                    status = Some(exit);
                    break;
                }
                Ok(None) => {}
                Err(_) => {
                    primary = Some(error(Code::Failed));
                    break;
                }
            }
            thread::sleep(Duration::from_millis(10));
        }
        drop(release);
        if status.is_none() {
            child.stop();
        }
        let write = writer.join().map_err(|_| error(Code::Failed))?;
        let read = reader.join().map_err(|_| error(Code::Failed))?;
        let diagnostics = errors.join().map_err(|_| error(Code::Failed))?;
        if let Some(error) = primary {
            return Err(error);
        }
        let result = read?;
        write?;
        if diagnostics? || !status.is_some_and(|status| status.success()) {
            return Err(error(Code::Failed));
        }
        check(cancel, deadline)?;
        Ok(result)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn response_framing_never_accepts_partial_or_extra_pdf_bytes() {
        let pdf = b"%PDF-test";
        let mut data = serde_json::to_vec(&serde_json::json!({"version":1,"ok":true,"pdfBytes":pdf.len(),"interactions":0,"elapsedMs":1})).unwrap();
        data.push(b'\n');
        data.extend_from_slice(pdf);
        assert_eq!(response(data.as_slice()).unwrap(), pdf);
        assert!(response(&data[..data.len() - 1]).is_err());
        data.push(0);
        assert!(response(data.as_slice()).is_err());
        assert_eq!(
            response(b"{\"version\":1,\"ok\":false,\"code\":\"encrypted\"}\n".as_slice())
                .unwrap_err()
                .code,
            Code::Encrypted
        );
        assert!(
            response(b"{\"version\":1,\"ok\":false,\"code\":\"source text\"}\n".as_slice())
                .is_err()
        );
    }
    #[test]
    fn workspaces_have_app_code_only_and_clean_their_owned_child() {
        let root = tempfile::tempdir().unwrap();
        let work = Work::new(&root.path().join("work")).unwrap();
        let path = work.path.clone();
        assert_eq!(fs::read(path.join("worker.py")).unwrap(), assets::WORKER);
        drop(work);
        assert!(!path.exists());
    }
    #[cfg(windows)]
    #[test]
    fn private_profile_paths_leave_room_for_the_engine_extension_registry() {
        let root = tempfile::tempdir().unwrap();
        let mut parent = root.path().join("profile");
        while crate::path_identity::normal(&parent).as_os_str().len() < 81 {
            parent.push("data");
        }
        let work = Work::new(&parent).unwrap();
        let registry = crate::path_identity::normal(&work.path).join("user/user/extensions/bundled/registry/com.sun.star.comp.deployment.component.PackageRegistryBackend/unorc");
        use std::os::windows::ffi::OsStrExt;
        assert!(
            registry.as_os_str().encode_wide().count() < 260,
            "An engine profile registry must fit Win32 path APIs"
        );
    }
}
