//! Own one verified local renderer and one bounded request. The caller still
//! authenticates source bytes and rechecks permissions/fingerprint afterwards.
use crate::office_assets::{self as assets, error, Catalog, Code, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant, SystemTime},
};

const WORK_OWNER: &[u8] = b"Prism Office preview worker v2\n";
const RECOVERY_AGE: Duration = Duration::from_secs(24 * 60 * 60);
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
                    phase: if n == self.0.catalog.file_count {
                        "finalizing"
                    } else {
                        "installing"
                    },
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
        let work = Work::new(&self.0.parent.join("work"), &|| check(cancel, deadline))?;
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
    lease: Option<File>,
}
impl Work {
    fn new(parent: &Path, check: assets::Check<'_>) -> Result<Self> {
        assets::mkdir(parent)?;
        let parent = crate::path_identity::canonical(parent).map_err(|_| error(Code::Integrity))?;
        recover_workspaces(&parent, check)?;
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
        let mut work = Self {
            parent,
            path,
            lease: None,
        };
        let mut lease = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(work.path.join("owner.txt"))
            .map_err(|_| error(Code::Failed))?;
        lease
            .write_all(WORK_OWNER)
            .map_err(|_| error(Code::Failed))?;
        // Independent shared leases in the parent, worker and Unix supervisor
        // overlap across startup/teardown. Recovery must obtain an exclusive
        // lease; a timestamp alone can never authorize deleting a live request.
        lease.try_lock_shared().map_err(|_| error(Code::Failed))?;
        work.lease = Some(lease);
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(work.path.join("worker.py"))
            .and_then(|mut file| file.write_all(assets::WORKER))
            .map_err(|_| error(Code::Failed))?;
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
        self.lease.take();
        // A supervisor can still be reaping descendants after a forced worker
        // stop. Normal cleanup obeys the same exclusive lease as crash recovery.
        let same_parent = crate::path_identity::canonical(&self.path)
            .is_ok_and(|path| path.parent() == Some(self.parent.as_path()));
        if same_parent {
            let _ = recover_workspace(
                &self.path,
                Instant::now() + Duration::from_secs(5),
                Duration::ZERO,
                &|| Ok(()),
            );
        }
    }
}
/// Best-effort crash recovery, only on the next original-page render. Old v1
/// directories have no cross-process lease contract and are never swept. A
/// day-long grace also leaves failed requests available for diagnosis and lets
/// kill-on-close OS teardown finish before a later process reclaims them.
fn recover_workspaces(parent: &Path, check: assets::Check<'_>) -> Result<usize> {
    let until = Instant::now() + Duration::from_millis(250);
    let mut removed = 0;
    let entries = match fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(_) => return Ok(0),
    };
    for entry in entries.take(512) {
        check()?;
        if Instant::now() >= until {
            break;
        }
        let Ok(entry) = entry else { continue };
        let name = entry.file_name();
        let Some(token) = name.to_str().and_then(|s| s.strip_prefix("r-")) else {
            continue;
        };
        if token.len() != 32
            || !token
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            continue;
        }
        let path = entry.path();
        if crate::path_identity::canonical(&path).ok().as_deref() != Some(path.as_path()) {
            continue;
        }
        if recover_workspace(&path, until, RECOVERY_AGE, check)? {
            removed += 1;
        }
    }
    Ok(removed)
}

fn recover_workspace(
    path: &Path,
    until: Instant,
    min_age: Duration,
    check: assets::Check<'_>,
) -> Result<bool> {
    let attempt = || -> Result<bool> {
        let marker = path.join("owner.txt");
        crate::path_identity::no_links(&marker).map_err(|_| error(Code::Integrity))?;
        let mut options = OpenOptions::new();
        options.read(true).write(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.custom_flags(0x00200000);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        }
        let mut lease = options.open(&marker).map_err(|_| error(Code::Integrity))?;
        // This nonblocking lock is shared with Python's flock/LockFileEx. A
        // suspended or long-running request remains protected even when old.
        lease.try_lock().map_err(|_| error(Code::Busy))?;
        let metadata = lease.metadata().map_err(|_| error(Code::Integrity))?;
        if !metadata.is_file()
            || metadata.len() != WORK_OWNER.len() as u64
            || SystemTime::now()
                .duration_since(metadata.modified().map_err(|_| error(Code::Integrity))?)
                .map_or(true, |age| age < min_age)
        {
            return Ok(false);
        }
        let mut owner = Vec::new();
        (&mut lease)
            .take(128)
            .read_to_end(&mut owner)
            .map_err(|_| error(Code::Integrity))?;
        if owner != WORK_OWNER {
            return Ok(false);
        }
        // Validate a bounded tree before removing anything. Never traverse a
        // symlink/junction; originals, engine stages and unknown directories
        // cannot be candidates. Keep the ownership marker until the last step
        // so an interrupted partial cleanup can be resumed on a later render.
        let mut paths = Vec::new();
        for entry in walkdir::WalkDir::new(path)
            .follow_links(false)
            .max_depth(64)
        {
            check()?;
            if Instant::now() >= until || paths.len() >= 4096 {
                return Ok(false);
            }
            let entry = entry.map_err(|_| error(Code::Integrity))?;
            // WalkDir truncates traversal at max_depth. A directory there may
            // hide more children, so reject it before deleting any sibling.
            if entry.depth() == 64 && entry.file_type().is_dir() {
                return Ok(false);
            }
            crate::path_identity::no_links(entry.path()).map_err(|_| error(Code::Integrity))?;
            if entry.path() != path && entry.path() != marker {
                paths.push((entry.path().to_owned(), entry.file_type().is_dir()));
            }
        }
        for (child, directory) in paths.into_iter().rev() {
            check()?;
            if Instant::now() >= until {
                return Ok(false);
            }
            crate::path_identity::no_links(&child).map_err(|_| error(Code::Integrity))?;
            if directory {
                fs::remove_dir(&child)
            } else {
                fs::remove_file(&child)
            }
            .map_err(|_| error(Code::Failed))?;
        }
        check()?;
        fs::remove_file(marker).map_err(|_| error(Code::Failed))?;
        drop(lease);
        fs::remove_dir(path).map_err(|_| error(Code::Failed))?;
        Ok(true)
    };
    match attempt() {
        Err(error) if matches!(error.code, Code::Cancelled | Code::TimedOut) => Err(error),
        result => Ok(result.unwrap_or(false)),
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

    fn recovery_fixture(parent: &Path, index: usize, owner: &[u8], old: bool) -> PathBuf {
        let path = parent.join(format!("r-{index:032x}"));
        fs::create_dir_all(path.join("user/nested")).unwrap();
        fs::write(path.join("user/nested/private-temp"), b"owned fixture").unwrap();
        let marker = path.join("owner.txt");
        fs::write(&marker, owner).unwrap();
        if old {
            File::options()
                .read(true)
                .write(true)
                .open(&marker)
                .unwrap()
                .set_modified(SystemTime::now() - RECOVERY_AGE - Duration::from_secs(60))
                .unwrap();
        }
        path
    }

    #[test]
    fn recovery_only_removes_old_inactive_v2_requests() {
        let root = tempfile::tempdir().unwrap();
        let parent = crate::path_identity::canonical(root.path()).unwrap();
        let old = recovery_fixture(&parent, 1, WORK_OWNER, true);
        let recent = recovery_fixture(&parent, 2, WORK_OWNER, false);
        let legacy = recovery_fixture(&parent, 3, b"Prism Office preview worker v1\n", true);
        let foreign = recovery_fixture(&parent, 4, b"another application", true);
        let wrong_name = recovery_fixture(&parent, 5, WORK_OWNER, true);
        fs::rename(&wrong_name, parent.join("not-a-request")).unwrap();
        let future = recovery_fixture(&parent, 6, WORK_OWNER, false);
        File::options()
            .write(true)
            .open(future.join("owner.txt"))
            .unwrap()
            .set_modified(SystemTime::now() + RECOVERY_AGE)
            .unwrap();
        assert_eq!(recover_workspaces(&parent, &|| Ok(())).unwrap(), 1);
        assert!(!old.exists());
        for path in [
            recent,
            legacy,
            foreign,
            parent.join("not-a-request"),
            future,
        ] {
            assert!(path.join("user/nested/private-temp").exists());
        }
        assert_eq!(recover_workspaces(&parent, &|| Ok(())).unwrap(), 0);
    }

    #[test]
    fn a_live_parent_lease_protects_even_an_old_request() {
        let root = tempfile::tempdir().unwrap();
        let work = Work::new(root.path(), &|| Ok(())).unwrap();
        work.lease
            .as_ref()
            .unwrap()
            .set_modified(SystemTime::now() - RECOVERY_AGE - Duration::from_secs(60))
            .unwrap();
        assert_eq!(recover_workspaces(&work.parent, &|| Ok(())).unwrap(), 0);
        assert!(work.path.join("worker.py").exists());
        let path = work.path.clone();
        drop(work);
        assert!(!path.exists());
    }

    #[test]
    fn cancelled_recovery_keeps_its_marker_and_can_resume() {
        let root = tempfile::tempdir().unwrap();
        let parent = crate::path_identity::canonical(root.path()).unwrap();
        let path = recovery_fixture(&parent, 1, WORK_OWNER, true);
        let calls = std::cell::Cell::new(0);
        let result = recover_workspace(
            &path,
            Instant::now() + Duration::from_secs(5),
            RECOVERY_AGE,
            &|| {
                calls.set(calls.get() + 1);
                if calls.get() >= 7 {
                    Err(error(Code::Cancelled))
                } else {
                    Ok(())
                }
            },
        );
        assert_eq!(result.unwrap_err().code, Code::Cancelled);
        assert_eq!(fs::read(path.join("owner.txt")).unwrap(), WORK_OWNER);
        assert!(!path.join("user/nested/private-temp").exists());
        assert_eq!(recover_workspaces(&parent, &|| Ok(())).unwrap(), 1);
    }

    #[test]
    fn a_depth_limit_does_not_authorize_partial_deletion_of_an_uninspected_tree() {
        let root = tempfile::tempdir().unwrap();
        let parent = crate::path_identity::canonical(root.path()).unwrap();
        let path = recovery_fixture(&parent, 1, WORK_OWNER, true);
        let mut deep = path.clone();
        for _ in 0..65 {
            deep.push("x");
        }
        fs::create_dir_all(&deep).unwrap();
        assert!(!recover_workspace(
            &path,
            Instant::now() + Duration::from_secs(5),
            RECOVERY_AGE,
            &|| Ok(())
        )
        .unwrap());
        assert!(path.join("user/nested/private-temp").exists());
        assert!(deep.is_dir());
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn recovery_does_not_follow_external_directory_links() {
        let root = tempfile::tempdir().unwrap();
        let parent = crate::path_identity::canonical(root.path()).unwrap();
        let outside = parent.join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("original"), b"must survive").unwrap();
        let path = recovery_fixture(&parent, 1, WORK_OWNER, true);
        let link = path.join("external");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        #[cfg(windows)]
        {
            let staging = parent.join("fixture-junction");
            crate::path_identity::junction(&staging, &outside);
            fs::rename(staging, &link).unwrap();
        }
        assert_eq!(recover_workspaces(&parent, &|| Ok(())).unwrap(), 0);
        assert_eq!(fs::read(outside.join("original")).unwrap(), b"must survive");
        assert!(path.join("user/nested/private-temp").exists());
        fs::remove_dir(&link)
            .or_else(|_| fs::remove_file(&link))
            .unwrap();
    }

    #[test]
    fn normal_cleanup_also_preserves_another_live_lease() {
        let root = tempfile::tempdir().unwrap();
        let work = Work::new(root.path(), &|| Ok(())).unwrap();
        let path = work.path.clone();
        let other = File::open(path.join("owner.txt")).unwrap();
        other.try_lock_shared().unwrap();
        drop(work);
        assert!(path.join("worker.py").exists());
        drop(other);
        assert!(recover_workspace(
            &path,
            Instant::now() + Duration::from_secs(5),
            Duration::ZERO,
            &|| Ok(())
        )
        .unwrap());
    }

    #[test]
    #[ignore = "opt-in actual Python/Rust cross-process lock interoperability"]
    fn python_worker_lease_blocks_recovery_until_process_exit() {
        let python =
            std::env::var_os("PRISM_OFFICE_QA_PYTHON").expect("explicit Python executable");
        let root = tempfile::tempdir().unwrap();
        let parent = crate::path_identity::canonical(root.path()).unwrap();
        let path = recovery_fixture(&parent, 1, WORK_OWNER, true);
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let worker = [
            "../worker.py",
            "../office-runtime/worker.py",
            "../tools/office-runtime/worker.py",
        ]
        .into_iter()
        .map(|path| manifest.join(path))
        .find(|path| path.is_file())
        .unwrap();
        let script = "import importlib.util,sys; from pathlib import Path; s=importlib.util.spec_from_file_location('w',sys.argv[1]); w=importlib.util.module_from_spec(s); s.loader.exec_module(w); lease=w.request_lease(Path(sys.argv[2])); print('ready',flush=True); sys.stdin.buffer.read(1)";
        let parent_lease = File::open(path.join("owner.txt")).unwrap();
        parent_lease.try_lock_shared().unwrap();
        let mut child = OwnedChild(
            Command::new(python)
                .args(["-B", "-c", script])
                .arg(worker)
                .arg(&path)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let mut ready = String::new();
        BufReader::new(child.0.stdout.take().unwrap())
            .read_line(&mut ready)
            .unwrap();
        assert_eq!(ready.trim(), "ready");
        drop(parent_lease);
        assert_eq!(recover_workspaces(&parent, &|| Ok(())).unwrap(), 0);
        child.0.kill().unwrap();
        child.0.wait().unwrap();
        assert_eq!(recover_workspaces(&parent, &|| Ok(())).unwrap(), 1);
    }

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
        let work = Work::new(&root.path().join("work"), &|| Ok(())).unwrap();
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
        let work = Work::new(&parent, &|| Ok(())).unwrap();
        let registry = crate::path_identity::normal(&work.path).join("user/user/extensions/bundled/registry/com.sun.star.comp.deployment.component.PackageRegistryBackend/unorc");
        use std::os::windows::ffi::OsStrExt;
        assert!(
            registry.as_os_str().encode_wide().count() < 260,
            "An engine profile registry must fit Win32 path APIs"
        );
    }
}
