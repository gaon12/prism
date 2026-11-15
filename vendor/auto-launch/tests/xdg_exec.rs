#![cfg(target_os = "linux")]

use auto_launch::{AutoLaunch, LinuxLaunchMode};
use std::{
    fs,
    os::unix::{ffi::OsStrExt, fs::PermissionsExt},
    path::PathBuf,
    process::Command,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const OWNER: &str = "Prism isolated desktop Exec regression v1\n";
const CHILD_ROOT: &str = "PRISM_DESKTOP_EXEC_TEST_ROOT";

// This explicitly selected check uses the real GIO desktop-entry launcher, not
// a shell parser standing in for it. Only an owned argv-recorder is launched.
#[test]
#[ignore = "requires Linux gio and dbus-run-session; launches an owned argv recorder"]
fn real_gio_preserves_executable_and_arguments() {
    let root = std::env::temp_dir().join(format!(
        "prism-desktop-exec-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&root).unwrap();
    fs::write(root.join("qa-owner.txt"), OWNER).unwrap();
    let root = root.canonicalize().unwrap();
    for mode in [
        "spaced_korean",
        "reserved_path",
        "reserved_without_percent",
        "equals_path",
        "literal_arguments",
        "wrapped_arguments",
        "simple",
    ] {
        let case = root.join(mode);
        let home = case.join("home");
        let runtime = case.join("runtime");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir(&runtime).unwrap();
        fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700)).unwrap();
        let output = Command::new("dbus-run-session")
            .arg("--")
            .arg(std::env::current_exe().unwrap())
            .args(["--exact", "isolated_gio_child", "--nocapture"])
            .env(CHILD_ROOT, &root)
            .env("PRISM_DESKTOP_EXEC_TEST_MODE", mode)
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", case.join("config"))
            .env("XDG_DATA_HOME", case.join("data"))
            .env("XDG_CACHE_HOME", case.join("cache"))
            .env("XDG_RUNTIME_DIR", &runtime)
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{mode}: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert_eq!(
        fs::read_to_string(root.join("qa-owner.txt")).unwrap(),
        OWNER
    );
    assert!(root
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("prism-desktop-exec-"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn isolated_gio_child() {
    let Some(root) = std::env::var_os(CHILD_ROOT) else {
        return;
    };
    let root = PathBuf::from(root).canonicalize().unwrap();
    assert_eq!(
        fs::read_to_string(root.join("qa-owner.txt")).unwrap(),
        OWNER
    );
    let mode = std::env::var("PRISM_DESKTOP_EXEC_TEST_MODE").unwrap();
    assert!([
        "spaced_korean",
        "reserved_path",
        "reserved_without_percent",
        "equals_path",
        "literal_arguments",
        "wrapped_arguments",
        "simple"
    ]
    .contains(&mode.as_str()));
    let case = root.join(&mode);
    assert_eq!(
        PathBuf::from(std::env::var_os("HOME").unwrap()),
        case.join("home")
    );
    assert_eq!(
        PathBuf::from(std::env::var_os("XDG_CONFIG_HOME").unwrap()),
        case.join("config")
    );
    let directory = case.join(if mode == "spaced_korean" {
        "앱 경로"
    } else {
        "bin"
    });
    fs::create_dir(&directory).unwrap();
    let filename = match mode.as_str() {
        "spaced_korean" => "Prism 읽기.AppImage",
        "reserved_path" => "Prism %f \"$'\\`&().AppImage",
        "reserved_without_percent" => "Prism \"$'\\`&().AppImage",
        "equals_path" => "Prism=읽기.AppImage",
        "wrapped_arguments" => "Prism %f.AppImage",
        _ => "prism-recorder",
    };
    let executable = directory.join(filename);
    fs::write(
        &executable,
        b"#!/bin/sh\nset -eu\nprintf '%s\\0' \"$0\" \"$@\" > \"$PRISM_EXEC_RECORD\"\n",
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    let args = if mode == "literal_arguments" || mode == "wrapped_arguments" {
        vec![
            "--background",
            "두 단어",
            "a\"b\\c$d`e'&",
            "%f",
            "literal%%",
            "$HOME $(printf not-expanded)",
            "",
            "line\nbreak",
            "tabs\tcarriage\rreturn",
        ]
    } else {
        vec!["--background"]
    };
    let app = AutoLaunch::new(
        "prism-exec-qa",
        executable.to_str().unwrap(),
        LinuxLaunchMode::XdgAutostart,
        &args,
    );
    app.enable().unwrap();
    let entry = case.join("config/autostart/prism-exec-qa.desktop");
    let record = case.join("argv.bin");
    let output = Command::new("gio")
        .arg("launch")
        .arg(&entry)
        .env("PRISM_EXEC_RECORD", &record)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "GIO rejected generated entry: {}\n{}",
        String::from_utf8_lossy(&output.stderr),
        fs::read_to_string(&entry).unwrap()
    );
    let mut expected = executable.as_os_str().as_bytes().to_vec();
    expected.push(0);
    for argument in args {
        expected.extend_from_slice(argument.as_bytes());
        expected.push(0);
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    let actual = loop {
        let actual = fs::read(&record).unwrap_or_default();
        if actual == expected || Instant::now() >= deadline {
            break actual;
        }
        thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(
        actual, expected,
        "GIO changed the executable path or argument boundaries"
    );
    app.disable().unwrap();
    assert!(!entry.exists());
}
