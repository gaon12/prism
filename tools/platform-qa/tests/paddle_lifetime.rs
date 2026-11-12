#![cfg(any(
    all(target_os = "linux", target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "aarch64")
))]
//! Actual production dispatch and lifetime test. The seed is an authenticated
//! official runtime, not an OCR result or server substitute. Only our Child is killed.
use prism_platform_qa::{ocr_assets, ocr_package};
#[cfg(target_os = "linux")]
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::{
    fs,
    os::unix::{fs::PermissionsExt, process::ExitStatusExt},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
#[path = "support/paddle_diagnostics.rs"]
mod diagnostics;
#[cfg(target_os = "macos")]
#[path = "support/macos_exit.rs"]
mod macos_exit;
#[path = "support/paddle_readiness.rs"]
mod readiness;
use diagnostics::{last_phase, FailureDiagnostics};
use readiness::ReadinessBudget;

struct Cleanup(Child);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn seed(source: &Path, data: &Path) {
    let catalog = ocr_assets::for_target(ocr_assets::target()).unwrap();
    let entries = ocr_package::load(source, &catalog).unwrap();
    let installed = data.join("ocr").join(catalog.directory());
    fs::create_dir_all(&installed).unwrap();
    for entry in &entries {
        let relative = ocr_assets::relative(&entry.path).unwrap();
        let destination = installed.join(relative);
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        let mut input = ocr_package::regular(&source.join(relative)).unwrap();
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&destination)
            .unwrap();
        assert_eq!(std::io::copy(&mut input, &mut output).unwrap(), entry.bytes);
    }
    fs::set_permissions(
        installed.join(&catalog.executable),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    fs::write(
        installed.join("inventory.json"),
        serde_json::to_vec(&entries).unwrap(),
    )
    .unwrap();
}

#[test]
#[ignore = "Requires PRISM_TEST_PADDLE_INSTALLED, the pinned complete native CPU runtime; Mac ARM64 also requires the explicit debug reference flag"]
fn real_paddle_auto_fallback_is_reaped_after_app_parent_sigkill() {
    let source = PathBuf::from(
        std::env::var_os("PRISM_TEST_PADDLE_INSTALLED").expect("Set PRISM_TEST_PADDLE_INSTALLED"),
    );
    let directory = tempfile::tempdir().unwrap();
    seed(&source, directory.path());
    let ready = directory.path().join("ready.json");
    let log = directory.path().join("fixture.log");
    let stderr = fs::File::create(&log).unwrap();
    let _diagnostics = FailureDiagnostics {
        ready: &ready,
        log: &log,
    };
    let mut parent = Cleanup(
        Command::new(env!("CARGO_BIN_EXE_prism-platform-qa"))
            .arg("--fixture-paddle-parent")
            .arg(directory.path())
            .arg(&ready)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(stderr)
            .spawn()
            .unwrap(),
    );
    let mut budget = ReadinessBudget::new(
        Instant::now(),
        prism_platform_qa::ocr::Limits::default().max_duration,
    );
    let status: serde_json::Value = loop {
        let phase_text = last_phase(&ready);
        let phase = serde_json::from_str::<serde_json::Value>(&phase_text).ok();
        assert!(
            budget.within_deadline(
                Instant::now(),
                phase.as_ref().and_then(|value| value["phase"].as_str()),
            ),
            "Actual Paddle recognition exceeded its bounded deadline; phase={phase_text}"
        );
        if let Ok(bytes) = fs::read(&ready) {
            if let Ok(value) = serde_json::from_slice(&bytes) {
                break value;
            }
        }
        assert!(
            parent.0.try_wait().unwrap().is_none(),
            "Actual Paddle parent exited before readiness; phase={}; inspect {}",
            last_phase(&ready),
            directory.path().display()
        );
        thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(status["autoFallback"], true);
    assert_eq!(status["actualKorean"], true);
    assert_eq!(status["sourceExtractionVerified"], true);
    let pid = status["pid"].as_u64().unwrap() as u32;
    // Attach while our parent fixture still owns the live worker. Neither
    // backend looks up a potentially reused PID after the parent is killed.
    #[cfg(target_os = "linux")]
    let worker = {
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
        assert!(fd >= 0);
        unsafe { OwnedFd::from_raw_fd(fd as i32) }
    };
    #[cfg(target_os = "macos")]
    let worker = macos_exit::ExitProbe::new(pid)
        .expect("retain the live owned worker's kernel exit notification");
    parent.0.kill().unwrap();
    assert_eq!(parent.0.wait().unwrap().signal(), Some(9));
    #[cfg(target_os = "linux")]
    {
        let mut ready = libc::pollfd {
            fd: worker.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        assert_eq!(unsafe { libc::poll(&mut ready, 1, 8000) }, 1);
        assert_ne!(
            ready.revents & libc::POLLIN,
            0,
            "The exact owned Paddle worker must exit after its app is killed"
        );
    }
    #[cfg(target_os = "macos")]
    worker
        .wait(Duration::from_secs(8))
        .expect("The exact owned Paddle worker must exit after its app is killed");
    println!(
        "Actual {} Paddle Auto recognition and owned worker exit after app SIGKILL passed",
        ocr_assets::target()
    );
}
