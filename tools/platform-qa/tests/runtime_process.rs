#![cfg(unix)]
//! Real child-process tests. Only directly spawned fixture handles are killed.
//! Synthetic localhost sockets and temporary files prove lifetime and isolation.
use prism_platform_qa::runtime_process::OwnedRuntimeProcess;
use serde_json::Value;
use std::{
    fs,
    net::{SocketAddr, TcpStream},
    path::Path,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const EXE: &str = env!("CARGO_BIN_EXE_prism-platform-qa");
struct Cleanup(Child);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn until(mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(8);
    while !check() {
        assert!(
            Instant::now() < deadline,
            "Owned fixture did not reach the expected state"
        );
        thread::sleep(Duration::from_millis(20));
    }
}
fn ready(path: &Path) -> Value {
    let mut value = None;
    until(|| {
        value = fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok());
        value.is_some()
    });
    value.unwrap()
}
fn address(data: &Value) -> SocketAddr {
    data["address"].as_str().unwrap().parse().unwrap()
}
fn connects(data: &Value) -> bool {
    TcpStream::connect_timeout(&address(data), Duration::from_millis(100)).is_ok()
}
fn process_exists(pid: u64) -> bool {
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "pid="])
        .output()
        .unwrap();
    output.status.success() && !String::from_utf8_lossy(&output.stdout).trim().is_empty()
}
fn server_command(directory: &Path, ready: &str) -> Command {
    let mut command = Command::new(EXE);
    command
        .args(["--fixture-server", ready, "$(literal); 한글 공백"])
        .current_dir(directory)
        .env("PRISM_SYNTHETIC_SERVER_KEY", "synthetic-only")
        .env_remove("PRISM_SHOULD_BE_ABSENT")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

#[test]
fn normal_stop_preserves_literal_arguments_env_cwd_and_unrelated_children() {
    let directory = tempfile::tempdir().unwrap();
    let mut owned = OwnedRuntimeProcess::spawn(
        server_command(directory.path(), "owned.json"),
        Path::new(EXE),
    )
    .unwrap();
    let owned_data = ready(&directory.path().join("owned.json"));
    assert_eq!(owned_data["pid"].as_u64().unwrap(), u64::from(owned.id()));
    assert_eq!(owned_data["literal"], "$(literal); 한글 공백");
    // macOS resolves /var temporary paths through /private/var in getcwd.
    assert_eq!(
        fs::canonicalize(owned_data["cwd"].as_str().unwrap()).unwrap(),
        fs::canonicalize(directory.path()).unwrap()
    );
    assert_eq!(owned_data["secretEnv"], true);
    assert_eq!(owned_data["removedEnv"], true);
    // Spawn this sibling AFTER the control pipe exists. It must not inherit a
    // writer that would prevent EOF, and it must survive the owned server stop.
    let mut sibling = Cleanup(
        server_command(directory.path(), "sibling.json")
            .spawn()
            .unwrap(),
    );
    let sibling_data = ready(&directory.path().join("sibling.json"));
    assert!(owned.stop().unwrap().success());
    until(|| !connects(&owned_data));
    assert!(connects(&sibling_data));
    assert!(sibling.0.try_wait().unwrap().is_none());
    drop(owned); // A second stop is harmless.
}

#[test]
fn forced_parent_exit_closes_the_pipe_and_reaps_only_its_owned_server() {
    use std::os::unix::process::ExitStatusExt;
    let directory = tempfile::tempdir().unwrap();
    let parent_ready = directory.path().join("parent.json");
    let mut parent = Cleanup(
        Command::new(EXE)
            .arg("--fixture-parent")
            .arg(directory.path())
            .arg(&parent_ready)
            .env("PRISM_SHOULD_BE_ABSENT", "synthetic-parent-only")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let server = ready(&directory.path().join("server.json"));
    assert_eq!(server["removedEnv"], true);
    until(|| {
        fs::read_to_string(&parent_ready)
            .is_ok_and(|value| value == server["pid"].as_u64().unwrap().to_string())
    });
    assert!(connects(&server));
    let group = |pid: u64| {
        let output = Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "pgid="])
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    };
    assert_ne!(
        group(u64::from(parent.0.id())),
        group(server["pid"].as_u64().unwrap())
    );
    parent.0.kill().unwrap();
    assert_eq!(parent.0.wait().unwrap().signal(), Some(9));
    until(|| !connects(&server));
    #[cfg(target_os = "linux")]
    until(|| !Path::new(&format!("/proc/{}", server["pid"].as_u64().unwrap())).exists());
}

#[test]
fn dropping_the_owner_stops_the_server_and_natural_exit_is_reported() {
    let directory = tempfile::tempdir().unwrap();
    let owned = OwnedRuntimeProcess::spawn(
        server_command(directory.path(), "drop.json"),
        Path::new(EXE),
    )
    .unwrap();
    let data = ready(&directory.path().join("drop.json"));
    drop(owned);
    until(|| !connects(&data));
    let mut command = Command::new(EXE);
    command.arg("--fixture-exit");
    let mut exited = OwnedRuntimeProcess::spawn(command, Path::new(EXE)).unwrap();
    let mut status = None;
    until(|| {
        status = exited.try_wait().unwrap();
        status.is_some()
    });
    assert_eq!(status.unwrap().code(), Some(23));
    assert_eq!(exited.stop().unwrap().code(), Some(23));
}

#[test]
fn bad_launcher_missing_program_and_relative_program_fail_without_leaking_a_server() {
    for (launcher, program) in [
        (EXE, "/prism-nonexistent-fixture-program"),
        (EXE, "relative"),
        ("/bin/true", "/bin/true"),
    ] {
        let before = Instant::now();
        assert!(OwnedRuntimeProcess::spawn(Command::new(program), Path::new(launcher)).is_err());
        assert!(before.elapsed() < Duration::from_secs(5));
    }
}

#[test]
fn an_already_closed_parent_pipe_cannot_leave_a_started_server_behind() {
    let directory = tempfile::tempdir().unwrap();
    let ready_path = directory.path().join("early.json");
    let mut owner = Cleanup(
        Command::new(EXE)
            .arg(prism_platform_qa::runtime_supervisor::FLAG)
            .arg(EXE)
            .arg("--")
            .arg("--fixture-server")
            .arg(&ready_path)
            .arg("literal")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    until(|| owner.0.try_wait().unwrap().is_some());
    if ready_path.exists() {
        let data = ready(&ready_path);
        until(|| !connects(&data));
        #[cfg(target_os = "linux")]
        until(|| !Path::new(&format!("/proc/{}", data["pid"].as_u64().unwrap())).exists());
    }
}

#[test]
fn an_unresponsive_launcher_cannot_make_startup_cleanup_wait_forever() {
    use std::{io, os::unix::fs::PermissionsExt};
    let directory = tempfile::tempdir().unwrap();
    let launcher = directory.path().join("unresponsive-launcher.sh");
    let pid_path = directory.path().join("unresponsive.pid");
    // This inert test-owned process deliberately ignores both the internal
    // protocol and stdin EOF. No user shell text or executable is involved.
    fs::write(
        &launcher,
        "#!/bin/sh\nprintf '%s' \"$$\" > \"$PRISM_UNRESPONSIVE_PID_FILE\"\nexec /bin/sleep 60\n",
    )
    .unwrap();
    fs::set_permissions(&launcher, fs::Permissions::from_mode(0o700)).unwrap();
    // Linux refuses to run a file that some process still has open for writing
    // (ETXTBSY). Tests run in parallel threads, so another test may fork right
    // while this script is being written; that child holds the write handle for
    // a moment, until its own exec closes it. Retry briefly in that case only.
    let mut attempts = 0;
    let (before, error) = loop {
        let before = Instant::now();
        let mut command = Command::new("/bin/true");
        command.env("PRISM_UNRESPONSIVE_PID_FILE", &pid_path);
        let error = OwnedRuntimeProcess::spawn(command, &launcher)
            .err()
            .unwrap();
        attempts += 1;
        if error.kind() != io::ErrorKind::ExecutableFileBusy || attempts == 20 {
            break (before, error);
        }
        thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    assert!(before.elapsed() < Duration::from_secs(18));
    let pid = fs::read_to_string(pid_path).unwrap().parse().unwrap();
    until(|| !process_exists(pid));
}

#[test]
#[ignore = "Requires PRISM_TEST_MODEL and PRISM_TEST_RUNTIME pinned host-platform assets"]
fn real_embedding_server_is_reaped_after_the_app_parent_is_killed() {
    use std::os::unix::process::ExitStatusExt;
    assert!(std::env::var_os("PRISM_TEST_MODEL").is_some());
    assert!(std::env::var_os("PRISM_TEST_RUNTIME").is_some());
    let directory = tempfile::tempdir().unwrap();
    let ready_path = directory.path().join("ready.json");
    let stderr = fs::File::create(directory.path().join("parent-stderr.log")).unwrap();
    let mut parent = Cleanup(
        Command::new(EXE)
            .arg("--fixture-model-parent")
            .arg(directory.path())
            .arg(&ready_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(stderr)
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(120);
    let status: Value = loop {
        if let Ok(bytes) = fs::read(&ready_path) {
            if let Ok(value) = serde_json::from_slice(&bytes) {
                break value;
            }
        }
        assert!(
            parent.0.try_wait().unwrap().is_none(),
            "Fixture exited before actual model readiness; inspect {}",
            directory.path().display()
        );
        assert!(
            Instant::now() < deadline,
            "Actual model fixture startup exceeded its bounded deadline"
        );
        thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(status["phase"], "running");
    assert_eq!(status["modelId"], "google/embeddinggemma-2");
    let pid = status["pid"].as_u64().unwrap();
    assert!(process_exists(pid));
    parent.0.kill().unwrap();
    assert_eq!(parent.0.wait().unwrap().signal(), Some(9));
    until(|| !process_exists(pid));
    println!(
        "Actual {} embedding server {pid} was reaped after SIGKILL of its owning app fixture",
        std::env::consts::OS
    );
}
