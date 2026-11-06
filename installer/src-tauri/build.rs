//! Prepares the setup program at build time:
//! 1. Reads Prism's version from the app's `tauri.conf.json`, so setup and app always match.
//! 2. Packs the built app into `payload.bin`, which `main.rs` embeds with `include_bytes!`.
//!
//! Which files are packed:
//! - `PRISM_PAYLOAD_DIR` set: every file in that folder (for apps that ship extra files).
//! - otherwise: `../../src-tauri/target/release/prism.exe` if it exists, together with the
//!   app's bundle resources (the component notices under `notices/`) that the app build
//!   copies beside it. The list comes from the app's `tauri.conf.json`, so the setup ships
//!   exactly what the app bundle declares. A release setup fails if any of them is missing.
//! - otherwise an empty payload. This keeps `cargo test` and UI work possible without building
//!   the app first; the setup then refuses to install and says the payload is missing.

#[path = "src/payload.rs"]
#[allow(dead_code)]
mod payload;
#[path = "src/safe_path.rs"]
#[allow(dead_code)]
mod safe_path;

use std::{
    env, fs,
    path::{Path, PathBuf},
};

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    // Keep the built-resource root free of `..` components. The payload path
    // validator deliberately rejects traversal before resolving filesystem links.
    let app_dir = manifest
        .parent()
        .and_then(Path::parent)
        .expect("setup manifest must be inside installer/src-tauri")
        .join("src-tauri");

    // 1. Version
    let config = app_dir.join("tauri.conf.json");
    println!("cargo:rerun-if-changed={}", config.display());
    let config_json = fs::read(&config)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .unwrap_or_default();
    let version = config_json["version"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| "0.0.0".into());
    println!("cargo:rustc-env=PRISM_VERSION={version}");

    // A tiny native worker removes the temporary uninstall copy without a shell script.
    let helper_source = manifest.join("src/cleanup_main.rs");
    for file in ["cleanup_main.rs", "temp_cleanup.rs", "safe_path.rs"] {
        println!(
            "cargo:rerun-if-changed={}",
            manifest.join("src").join(file).display()
        );
    }
    let helper = PathBuf::from(env::var("OUT_DIR").unwrap()).join("prism-cleanup.exe");
    let compiler = env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let status = std::process::Command::new(compiler)
        .arg(&helper_source)
        .args([
            "--edition=2021",
            "-C",
            "opt-level=s",
            "-C",
            "panic=abort",
            "-C",
            "strip=symbols",
        ])
        .arg("--target")
        .arg(env::var("TARGET").unwrap())
        .arg("-o")
        .arg(helper)
        .status()
        .expect("compile native cleanup worker");
    assert!(status.success(), "native cleanup worker did not compile");

    // 2. Payload
    println!("cargo:rerun-if-env-changed=PRISM_PAYLOAD_DIR");
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    if let Ok(dir) = env::var("PRISM_PAYLOAD_DIR") {
        let root = safe_path::root(Path::new(&dir)).expect("validate payload source folder");
        collect(&root, &root, &mut files).expect("read payload source files");
    } else {
        let exe = app_dir.join("target/release/prism.exe");
        println!("cargo:rerun-if-changed={}", exe.display());
        if let Ok(bytes) = fs::read(&exe) {
            files.push(("prism.exe".into(), bytes));
            // The resources the app declares (component notices), copied beside the exe by
            // the app build. Required for a release setup; optional for local debug builds.
            let release = env::var("PROFILE").as_deref() == Ok("release");
            let built = app_dir.join("target/release");
            for name in bundle_resources(&config_json) {
                println!("cargo:rerun-if-changed={}", built.join(&name).display());
                match resource_bytes(&built, &name) {
                    Ok(bytes) => files.push((name, bytes)),
                    Err(error) if release => {
                        panic!("A release setup must include the built resource {name}: {error}")
                    }
                    Err(_) => println!("cargo:warning=Built resource {name} not found; skipped."),
                }
            }
        }
    }
    if files.is_empty() {
        assert!(
            env::var("PROFILE").as_deref() != Ok("release"),
            "A release setup must contain the built Prism executable."
        );
        println!("cargo:warning=No Prism build found; the setup will contain no app files.");
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    assert!(
        files.is_empty() || files.iter().any(|(name, _)| name == "prism.exe"),
        "Payload must contain prism.exe"
    );
    let payload_size: u64 = files.iter().map(|(_, bytes)| bytes.len() as u64).sum();
    println!("cargo:rustc-env=PRISM_PAYLOAD_SIZE={payload_size}");
    let borrowed: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
        .collect();
    let packed = payload::pack(&borrowed).expect("pack payload");
    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("payload.bin");
    fs::write(out, packed).expect("write payload");

    // Our own manifest: run as the current user (no admin prompt), per-monitor DPI.
    let windows =
        tauri_build::WindowsAttributes::new().app_manifest(include_str!("setup.manifest"));
    tauri_build::try_build(tauri_build::Attributes::new().windows_attributes(windows))
        .expect("tauri build");
}

/// The app's declared bundle resources as "/"-separated relative names. Only plain relative
/// file names are accepted (no globs, no parent folders), so nothing outside the build
/// folder can be packed.
fn bundle_resources(config: &serde_json::Value) -> Vec<String> {
    config["bundle"]["resources"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    let name = item
                        .as_str()
                        .expect("bundle resources must be plain file names")
                        .replace('\\', "/");
                    assert!(
                        !name.contains(['*', '?', '[']),
                        "bundle resource globs are not supported by the setup"
                    );
                    safe_path::relative_name(&name).expect("bundle resource name is unsafe");
                    name
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Reads one built resource exactly as built: a regular file (no links) inside `root`,
/// not empty and at most 16 MiB.
fn resource_bytes(root: &Path, name: &str) -> std::io::Result<Vec<u8>> {
    let path = safe_path::file(root, name)?;
    safe_path::no_links(&path)?;
    let metadata = fs::symlink_metadata(&path)?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > 16 * 1024 * 1024 {
        return Err(std::io::Error::other("not a regular non-empty file"));
    }
    fs::read(path)
}

/// Adds every file below `dir` with a "/"-separated name relative to `root`.
fn collect(root: &Path, dir: &Path, files: &mut Vec<(String, Vec<u8>)>) -> std::io::Result<()> {
    safe_path::no_links(dir)?;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        println!("cargo:rerun-if-changed={}", path.display());
        safe_path::no_links(&path)?;
        if entry.file_type()?.is_dir() {
            collect(root, &path, files)?;
        } else if entry.file_type()?.is_file() {
            let relative = path.strip_prefix(root).map_err(std::io::Error::other)?;
            let name = relative.to_string_lossy().replace('\\', "/");
            safe_path::relative_name(&name)?;
            files.push((name, fs::read(path)?));
        } else {
            return Err(std::io::Error::other("Unsupported payload entry."));
        }
    }
    Ok(())
}
