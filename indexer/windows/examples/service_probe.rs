//! Explicit ordinary-user verification of the actual installed service. No
//! installation/elevation, synthetic responses, app profile or clipboard use.
//! --churn creates only its own new temporary files and removes them on exit.
#[cfg(windows)]
fn main() {
    if let Err(error) = native::run() {
        eprintln!("{{\"errorKind\":\"{:?}\"}}", error.kind());
        std::process::exit(2);
    }
}
#[cfg(not(windows))]
fn main() {
    eprintln!("This installed-service probe requires Windows.");
    std::process::exit(2);
}

#[cfg(windows)]
mod native {
    use prism_ntfs_protocol::{stream::Event, DriveLetter};
    use prism_ntfs_windows::{administration, client};
    use std::os::windows::ffi::OsStrExt;
    use std::{
        fs,
        io::{self, Write},
        path::{Path, PathBuf},
        sync::{atomic::AtomicBool, Arc},
        time::{Instant, SystemTime, UNIX_EPOCH},
    };
    use windows_sys::Win32::Storage::FileSystem::MoveFileW;

    struct Report {
        ms: u128,
        paths: u64,
        found: Vec<Option<(u64, Option<i64>)>>,
    }
    fn snapshot(drive: DriveLetter, targets: &[PathBuf]) -> io::Result<Report> {
        let targets: Vec<_> = targets
            .iter()
            .map(|path| path.to_string_lossy().replace('/', "\\"))
            .collect();
        let mut report = Report {
            ms: 0,
            paths: 0,
            found: vec![None; targets.len()],
        };
        let start = Instant::now();
        client::snapshot(
            Path::new(&drive.root()),
            Arc::new(AtomicBool::new(false)),
            |event| {
                if let Event::Records(records) = event {
                    report.paths += records.len() as u64;
                    for record in records {
                        if let Some(index) = targets
                            .iter()
                            .position(|path| path.eq_ignore_ascii_case(&record.path))
                        {
                            report.found[index] = Some((record.size, record.modified_ms));
                        }
                    }
                }
                Ok(())
            },
        )?;
        report.ms = start.elapsed().as_millis();
        Ok(report)
    }
    fn print(label: &str, report: &Report) {
        println!(
            "{{\"phase\":\"{label}\",\"ms\":{},\"paths\":{},\"ordinaryClient\":true}}",
            report.ms, report.paths
        );
    }
    fn invalid() -> io::Error {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "Owned fixture update was not reflected.",
        )
    }
    fn require(condition: bool) -> io::Result<()> {
        if condition {
            Ok(())
        } else {
            Err(invalid())
        }
    }
    struct Fixture {
        root: PathBuf,
        files: Vec<PathBuf>,
        directories: Vec<PathBuf>,
    }
    impl Fixture {
        fn new(drive: DriveLetter) -> io::Result<Self> {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(io::Error::other)?
                .as_nanos();
            let created = std::env::temp_dir().join(format!(
                "prism-service-probe-{}-{nonce}",
                std::process::id()
            ));
            fs::create_dir(&created)?;
            let canonical = match fs::canonicalize(&created) {
                Ok(path) => path,
                Err(error) => {
                    let _ = fs::remove_dir(&created);
                    return Err(error);
                }
            };
            let value = canonical.to_string_lossy();
            let root = PathBuf::from(value.strip_prefix(r"\\?\").unwrap_or(&value));
            if !root
                .to_string_lossy()
                .get(..3)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(&drive.root()))
            {
                let _ = fs::remove_dir(&created);
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "The probe's temporary directory must be on the requested drive.",
                ));
            }
            Ok(Self {
                directories: vec![root.clone()],
                root,
                files: vec![],
            })
        }
        fn file(&mut self, relative: &str, body: &[u8]) -> io::Result<PathBuf> {
            let path = self.root.join(relative);
            let mut file = fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&path)?;
            self.files.push(path.clone());
            file.write_all(body)?;
            file.sync_all()?;
            Ok(path)
        }
        fn directory(&mut self, relative: &str) -> io::Result<PathBuf> {
            let path = self.root.join(relative);
            fs::create_dir(&path)?;
            self.directories.push(path.clone());
            Ok(path)
        }
        fn rename(&mut self, from: &Path, to: &Path) -> io::Result<()> {
            let wide = |path: &Path| {
                path.as_os_str()
                    .encode_wide()
                    .chain(Some(0))
                    .collect::<Vec<_>>()
            };
            // Unlike replacing rename APIs, this must fail if the destination
            // is occupied. Never replace unrelated data in a probe.
            if unsafe { MoveFileW(wide(from).as_ptr(), wide(to).as_ptr()) } == 0 {
                return Err(io::Error::last_os_error());
            }
            for path in self.files.iter_mut().chain(self.directories.iter_mut()) {
                if let Ok(tail) = path.strip_prefix(from) {
                    *path = to.join(tail);
                }
            }
            Ok(())
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            for path in &self.files {
                let _ = fs::remove_file(path);
            }
            // Nonrecursive cleanup preserves anything not created by us.
            for path in self.directories.iter().rev() {
                let _ = fs::remove_dir(path);
            }
        }
    }
    fn churn(drive: DriveLetter) -> io::Result<()> {
        let mut fixture = Fixture::new(drive)?;
        let keep = fixture.file("유지-한글.md", b"keep")?;
        let removed = fixture.file("removed.md", b"remove")?;
        let before = fixture.file("before.txt", b"rename")?;
        let old_parent = fixture.directory("old-parent")?;
        let child = fixture.file("old-parent/child-문서.md", b"child")?;
        let initial = snapshot(
            drive,
            &[keep.clone(), removed.clone(), before.clone(), child.clone()],
        )?;
        require(initial.found.iter().all(Option::is_some))?;
        print("beforeChurn", &initial);
        fs::remove_file(&removed)?;
        fixture.files.retain(|path| path != &removed);
        let after = fixture.root.join("after.txt");
        fixture.rename(&before, &after)?;
        let new_parent = fixture.root.join("new-parent");
        fixture.rename(&old_parent, &new_parent)?;
        fs::write(&keep, b"updated file length")?;
        let moved_child = new_parent.join("child-문서.md");
        let changed = snapshot(drive, &[keep, removed, before, after, child, moved_child])?;
        require(changed.found[0].is_some_and(|(size, modified)| {
            size == b"updated file length".len() as u64 && modified.is_some()
        }))?;
        require(
            changed.found[1].is_none()
                && changed.found[2].is_none()
                && changed.found[3].is_some()
                && changed.found[4].is_none()
                && changed.found[5].is_some(),
        )?;
        print("afterChurn", &changed);
        println!("{{\"churnVerified\":true,\"unicode\":true,\"rename\":true,\"delete\":true,\"directoryDescendants\":true,\"updatedSize\":true}}");
        Ok(())
    }
    pub(super) fn run() -> io::Result<()> {
        if administration::is_elevated()? {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "This verification must run without elevation.",
            ));
        }
        let args: Vec<_> = std::env::args().skip(1).collect();
        if !(args.len() == 2 || args.len() == 3)
            || args[0] != "--drive"
            || args[1].len() != 1
            || args
                .get(2)
                .is_some_and(|value| value != "--churn" && value != "--parallel")
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Usage: service_probe --drive C [--churn|--parallel]",
            ));
        }
        let drive = DriveLetter::new(args[1].as_bytes()[0])?;
        match args.get(2).map(String::as_str) {
            Some("--churn") => churn(drive),
            Some("--parallel") => {
                let reports = std::thread::scope(|scope| {
                    let first = scope.spawn(|| snapshot(drive, &[]));
                    let second = scope.spawn(|| snapshot(drive, &[]));
                    (first.join(), second.join())
                });
                let first = reports
                    .0
                    .map_err(|_| io::Error::other("Probe thread failed."))??;
                let second = reports
                    .1
                    .map_err(|_| io::Error::other("Probe thread failed."))??;
                print("parallel1", &first);
                print("parallel2", &second);
                Ok(())
            }
            _ => {
                print("snapshot", &snapshot(drive, &[])?);
                Ok(())
            }
        }
    }
}
