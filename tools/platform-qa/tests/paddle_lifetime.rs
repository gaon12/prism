#![cfg(all(target_os = "linux", target_arch = "x86_64"))]
//! Actual production dispatch and lifetime test. The seed is an authenticated
//! official runtime, not an OCR result or server substitute. Only our Child is killed.
use prism_platform_qa::{ocr_assets, ocr_package};
use std::{
    fs,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::{fs::PermissionsExt, process::ExitStatusExt},
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

struct Cleanup(Child);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn seed(source: &Path, data: &Path) {
    let catalog = ocr_assets::for_target("linux-x64").unwrap();
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
#[ignore = "Requires PRISM_TEST_PADDLE_INSTALLED, the pinned complete Linux CPU runtime"]
fn real_paddle_auto_fallback_is_reaped_after_app_parent_sigkill() {
    let source = PathBuf::from(
        std::env::var_os("PRISM_TEST_PADDLE_INSTALLED").expect("Set PRISM_TEST_PADDLE_INSTALLED"),
    );
    let directory = tempfile::tempdir().unwrap();
    seed(&source, directory.path());
    let ready = directory.path().join("ready.json");
    let stderr = fs::File::create(directory.path().join("fixture.log")).unwrap();
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
    let deadline = Instant::now() + Duration::from_secs(180);
    let status: serde_json::Value = loop {
        if let Ok(bytes) = fs::read(&ready) {
            if let Ok(value) = serde_json::from_slice(&bytes) {
                break value;
            }
        }
        assert!(
            parent.0.try_wait().unwrap().is_none(),
            "Actual Paddle parent exited before readiness; inspect {}",
            directory.path().display()
        );
        assert!(
            Instant::now() < deadline,
            "Actual Paddle recognition exceeded its bounded deadline"
        );
        thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(status["autoFallback"], true);
    assert_eq!(status["actualKorean"], true);
    let pid = status["pid"].as_u64().unwrap() as u32;
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    assert!(fd >= 0);
    let worker = unsafe { OwnedFd::from_raw_fd(fd as i32) };
    parent.0.kill().unwrap();
    assert_eq!(parent.0.wait().unwrap().signal(), Some(9));
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
    println!("Actual Linux Paddle Auto recognition and owned worker exit after app SIGKILL passed");
}
