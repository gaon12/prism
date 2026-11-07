#![cfg(target_os = "linux")]

use auto_launch::{AutoLaunch, LinuxLaunchMode};
use std::{
    fs,
    path::PathBuf,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

const OWNER: &str = "Prism isolated XDG autostart regression v1\n";
const CHILD_ROOT: &str = "PRISM_AUTOSTART_TEST_ROOT";

// Environment variables belong to each fresh child, never the parallel test
// runner. All startup files stay below a newly created, marked fixture directory.
#[test]
fn xdg_paths_and_home_fallback() {
    let root = std::env::temp_dir().join(format!(
        "prism-autostart-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&root).unwrap();
    fs::write(root.join("qa-owner.txt"), OWNER).unwrap();
    let root = root.canonicalize().unwrap();
    for mode in ["absolute", "unset", "empty", "relative"] {
        let case = root.join(mode);
        let home = case.join("home");
        fs::create_dir_all(&home).unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap());
        child
            .args(["--exact", "isolated_child", "--nocapture"])
            .env(CHILD_ROOT, &root)
            .env("PRISM_AUTOSTART_TEST_MODE", mode)
            .env("HOME", &home);
        match mode {
            "absolute" => {
                child.env("XDG_CONFIG_HOME", case.join("설정 config"));
            }
            "empty" => {
                child.env("XDG_CONFIG_HOME", "");
            }
            "relative" => {
                child.env("XDG_CONFIG_HOME", "relative-config");
            }
            _ => {
                child.env_remove("XDG_CONFIG_HOME");
            }
        }
        let output = child.output().unwrap();
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
    // This exact directory was created above; no parent/home path is removed.
    assert!(root
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("prism-autostart-"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn isolated_child() {
    let Some(root) = std::env::var_os(CHILD_ROOT) else {
        return;
    };
    let root = PathBuf::from(root).canonicalize().unwrap();
    assert_eq!(
        fs::read_to_string(root.join("qa-owner.txt")).unwrap(),
        OWNER
    );
    let mode = std::env::var("PRISM_AUTOSTART_TEST_MODE").unwrap();
    assert!(["absolute", "unset", "empty", "relative"].contains(&mode.as_str()));
    let case = root.join(&mode);
    let home = PathBuf::from(std::env::var_os("HOME").unwrap())
        .canonicalize()
        .unwrap();
    assert_eq!(home, case.join("home"));
    let default_config = home.join(".config");
    let custom_config = case.join("설정 config");
    for directory in [&default_config, &custom_config] {
        fs::create_dir_all(directory.join("autostart")).unwrap();
        fs::write(directory.join("autostart/unrelated.desktop"), "preserve").unwrap();
    }
    let (selected, other) = if mode == "absolute" {
        (&custom_config, &default_config)
    } else {
        (&default_config, &custom_config)
    };
    let entry = selected.join("autostart/prism-xdg-qa.desktop");
    let other_entry = other.join("autostart/prism-xdg-qa.desktop");
    fs::write(&other_entry, "other config sentinel").unwrap();
    let app = AutoLaunch::new(
        "prism-xdg-qa",
        "/opt/prism.AppImage",
        LinuxLaunchMode::XdgAutostart,
        &["--background"],
    );
    assert!(
        !app.is_enabled().unwrap(),
        "the other config must not count as enabled"
    );
    app.enable().unwrap();
    assert!(app.is_enabled().unwrap());
    let generated = fs::read_to_string(&entry).unwrap();
    assert!(generated.contains("Exec=/opt/prism.AppImage --background"));
    assert!(generated.contains("Type=Application"));
    assert_eq!(
        fs::read_to_string(&other_entry).unwrap(),
        "other config sentinel"
    );
    app.disable().unwrap();
    app.disable().unwrap();
    assert!(!app.is_enabled().unwrap());
    assert!(!entry.exists());
    assert_eq!(
        fs::read_to_string(&other_entry).unwrap(),
        "other config sentinel"
    );
    for directory in [&default_config, &custom_config] {
        assert_eq!(
            fs::read_to_string(directory.join("autostart/unrelated.desktop")).unwrap(),
            "preserve"
        );
    }
    assert!(!case.join("relative-config").exists());
}
