#![cfg(target_os = "linux")]
//! Real renderer subprocesses and original PDF pixels. No model, Python,
//! desktop, service, fake renderer result or user document is required.
use prism_platform_qa::{ocr::Code, pdf_renderer};
use std::{
    fs,
    io::{Read, Write},
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    path::Path,
    process::{Child, Command, Stdio},
    sync::atomic::AtomicBool,
    thread,
    time::{Duration, Instant},
};
const EXE: &str = env!("CARGO_BIN_EXE_prism-platform-qa");
fn capture(name: &str, bytes: &[u8]) {
    if let Some(directory) = std::env::var_os("PRISM_TEST_PDF_CAPTURE_DIR") {
        fs::create_dir_all(&directory).unwrap();
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(Path::new(&directory).join(name))
            .unwrap()
            .write_all(bytes)
            .unwrap();
    }
}
struct Cleanup(Child);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn preview(bytes: &[u8], page: u32) -> Result<Vec<u8>, prism_platform_qa::ocr::Error> {
    pdf_renderer::render(
        bytes,
        page,
        480,
        Path::new(EXE),
        &AtomicBool::new(false),
        Instant::now() + Duration::from_secs(20),
    )
}
#[test]
fn actual_renderer_preserves_crop_rotation_scans_text_blank_and_source_bytes() {
    let crop = include_bytes!("../../../tests/fixtures/ocr/crop-clockwise.pdf");
    let png = preview(crop, 1).unwrap();
    capture("crop-rotation.png", &png);
    let image = image::load_from_memory(&png).unwrap().to_rgba8();
    assert_eq!(image.dimensions(), (100, 200));
    assert_eq!(image.get_pixel(50, 25).0, [255, 0, 0, 255]);
    assert_eq!(image.get_pixel(50, 175).0, [0, 0, 255, 255]);
    assert_eq!(preview(crop, 2).unwrap_err().code, Code::InvalidInput);
    assert_eq!(
        preview(b"damaged", 1).unwrap_err().code,
        Code::CorruptDocument
    );
    let mixed = include_bytes!("../../../tests/fixtures/ocr/mixed.pdf");
    for page in 1..=4 {
        let output = preview(mixed, page).unwrap();
        capture(&format!("original-page-{page}.png"), &output);
        let image = image::load_from_memory(&output).unwrap().to_rgba8();
        let ink = image
            .pixels()
            .filter(|p| p[0] < 180 && p[1] < 180 && p[2] < 180)
            .count();
        if page == 4 {
            assert_eq!(ink, 0);
        } else {
            assert!(
                ink > 50,
                "Original page {page} must contain real visible ink"
            );
        }
    }
    // Decrypt only in memory with the existing application dependency, just as
    // document_extract does for OCR. The renderer receives no password/path.
    let source = include_bytes!("../../../tests/fixtures/ocr/scan-locked.pdf");
    for password in ["ocr-fixture-password", "ocr-fixture-owner"] {
        let options = pdf_extract::LoadOptions::with_password(password);
        let mut document = pdf_extract::Document::load_mem_with_options(source, options).unwrap();
        let mut plaintext = Vec::new();
        document.save_to(&mut plaintext).unwrap();
        let output = preview(&plaintext, 1).unwrap();
        assert!(image::load_from_memory(&output).unwrap().width() > 0);
    }
    assert!(source.starts_with(b"%PDF"));
    assert_eq!(preview(source, 1).unwrap_err().code, Code::CorruptDocument);
}
#[test]
fn actual_collection_media_uses_renderer_without_enabling_ocr() {
    let directory = tempfile::tempdir().unwrap();
    let output = Command::new(EXE)
        .arg("--fixture-pdf-collection")
        .arg(directory.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let bytes = fs::read(directory.path().join("actual-cover.webp")).unwrap();
    capture("collection-cover.webp", &bytes);
    assert_eq!(&bytes[8..12], b"WEBP");
    let image = image::load_from_memory(&bytes).unwrap().to_rgba8();
    assert_eq!(image.dimensions(), (100, 200));
    assert_eq!(image.get_pixel(50, 25).0, [255, 0, 0, 255]);
    assert_eq!(image.get_pixel(50, 175).0, [0, 0, 255, 255]);
    println!("{}", String::from_utf8_lossy(&output.stdout));
}
fn waiting_worker() -> Cleanup {
    let mut child = Cleanup(
        Command::new(EXE)
            .arg(pdf_renderer::FLAG)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let mut ready = [0; 9];
    child
        .0
        .stdout
        .as_mut()
        .unwrap()
        .read_exact(&mut ready)
        .unwrap();
    assert_eq!(&ready, b"PRISMPDF0");
    child
}
#[test]
fn guarded_worker_enforces_bounded_protocol_and_exits_on_parent_death() {
    let directory = tempfile::tempdir().unwrap();
    let ready = directory.path().join("ready.json");
    let mut sibling = waiting_worker();
    let mut parent = Cleanup(
        Command::new(EXE)
            .arg("--fixture-pdf-parent")
            .arg(&ready)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(8);
    let pid = loop {
        if let Ok(bytes) = fs::read(&ready) {
            if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                break value["pid"].as_u64().unwrap() as u32;
            }
        }
        assert!(parent.0.try_wait().unwrap().is_none());
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    };
    let limits = fs::read_to_string(format!("/proc/{pid}/limits")).unwrap();
    assert!(limits.contains("2147483648")); // 2GiB address-space ceiling.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    assert!(fd >= 0);
    let process = unsafe { OwnedFd::from_raw_fd(fd as i32) };
    parent.0.kill().unwrap();
    parent.0.wait().unwrap();
    let mut polled = libc::pollfd {
        fd: process.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    assert_eq!(unsafe { libc::poll(&mut polled, 1, 5000) }, 1);
    assert!(
        sibling.0.try_wait().unwrap().is_none(),
        "Other directly-owned workers must survive"
    );
    // A header announcing >128MiB is rejected without allocating its payload.
    let mut oversized = b"PRISMPDF1".to_vec();
    oversized.extend_from_slice(&1u32.to_be_bytes());
    oversized.extend_from_slice(&480u32.to_be_bytes());
    oversized.extend_from_slice(&((128u64 << 20) + 1).to_be_bytes());
    let mut stdin = sibling.0.stdin.take().unwrap();
    stdin.write_all(&oversized).unwrap();
    drop(stdin);
    let mut response = Vec::new();
    sibling
        .0
        .stdout
        .take()
        .unwrap()
        .read_to_end(&mut response)
        .unwrap();
    assert_eq!(&response[..9], b"PRISMPNG1");
    assert_eq!(response[9], 1);
    assert!(sibling.0.wait().unwrap().success());
}
#[test]
fn pre_cancelled_expired_and_bad_requests_do_not_launch_a_program() {
    for (cancel, deadline, expected) in [
        (
            true,
            Instant::now() + Duration::from_secs(5),
            Code::Cancelled,
        ),
        (
            false,
            Instant::now() - Duration::from_secs(1),
            Code::TimedOut,
        ),
    ] {
        assert_eq!(
            pdf_renderer::render(
                b"x",
                1,
                480,
                Path::new("/no-such-executable"),
                &AtomicBool::new(cancel),
                deadline
            )
            .unwrap_err()
            .code,
            expected
        );
    }
    assert_eq!(preview(b"", 1).unwrap_err().code, Code::InvalidInput);
}

#[test]
fn cancellation_during_large_pipe_transfer_reaps_the_exact_owned_child() {
    use std::sync::atomic::Ordering;
    use std::sync::mpsc;
    // Deliberately invalid synthetic bytes exercise cancellation of a blocked
    // 128MiB input pipe, not an invented successful rendering result.
    let bytes = vec![0; 128 << 20];
    let cancel = AtomicBool::new(false);
    let (started, receive) = mpsc::sync_channel(1);
    thread::scope(|scope| {
        let worker = scope.spawn(|| {
            let tid = unsafe { libc::syscall(libc::SYS_gettid) };
            started.send(tid).unwrap();
            pdf_renderer::render(
                &bytes,
                1,
                480,
                Path::new(EXE),
                &cancel,
                Instant::now() + Duration::from_secs(10),
            )
        });
        let tid = receive.recv().unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let pid = loop {
            let children = fs::read_to_string(format!("/proc/self/task/{tid}/children")).unwrap();
            if let Some(pid) = children.split_whitespace().next() {
                break pid.parse::<u32>().unwrap();
            }
            assert!(
                !worker.is_finished(),
                "Renderer must still own the transfer"
            );
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        };
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
        assert!(fd >= 0);
        let process = unsafe { OwnedFd::from_raw_fd(fd as i32) };
        cancel.store(true, Ordering::Release);
        assert_eq!(worker.join().unwrap().unwrap_err().code, Code::Cancelled);
        let mut polled = libc::pollfd {
            fd: process.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        assert_eq!(
            unsafe { libc::poll(&mut polled, 1, 0) },
            1,
            "Cancellation returns only after its own child really exits"
        );
    });
    assert_eq!(
        pdf_renderer::render(
            &bytes,
            1,
            480,
            Path::new(EXE),
            &AtomicBool::new(false),
            Instant::now() + Duration::from_millis(10)
        )
        .unwrap_err()
        .code,
        Code::TimedOut
    );
}
