fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        build_index_helper();
    }
    tauri_build::build()
}

/// Build both images from this checkout. The one-shot helper embeds the host;
/// the app embeds the helper. Packaging a portable app therefore cannot omit
/// the matching service installer or pick up an unrelated stale executable.
fn build_index_helper() {
    use std::{env, path::PathBuf, process::Command};
    let project = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let indexer = project.join("../indexer");
    for path in [
        "Cargo.toml",
        "Cargo.lock",
        "protocol/src",
        "core/src",
        "windows/src",
        "windows/Cargo.toml",
        "service/src",
        "service/Cargo.toml",
        "service/build.rs",
    ] {
        println!("cargo:rerun-if-changed={}", indexer.join(path).display());
    }
    let target = env::var("TARGET").unwrap();
    // This target belongs to this exact app build, not to another source copy.
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("index-service");
    let build = |binary: &str, payload: Option<&std::path::Path>| {
        let mut command = Command::new(env::var_os("CARGO").unwrap());
        command
            .args([
                "build",
                "--locked",
                "--release",
                "--jobs",
                "1",
                "--manifest-path",
            ])
            .arg(indexer.join("Cargo.toml"))
            .args([
                "-p",
                "prism-file-index",
                "--bin",
                binary,
                "--target",
                &target,
            ])
            .env("CARGO_TARGET_DIR", &output)
            .env_remove("CARGO_MAKEFLAGS");
        if let Some(payload) = payload {
            command
                .args(["--features", "admin"])
                .env("PRISM_INDEX_SERVICE_PAYLOAD", payload);
        } else {
            command.env_remove("PRISM_INDEX_SERVICE_PAYLOAD");
        }
        assert!(
            command
                .status()
                .expect("Run the index-service build.")
                .success(),
            "Could not build {binary}."
        );
    };
    let binaries = output.join(&target).join("release");
    build("prism-file-index", None);
    build(
        "prism-file-index-admin",
        Some(&binaries.join("prism-file-index.exe")),
    );
    let helper = binaries.join("prism-file-index-admin.exe");
    println!(
        "cargo:rustc-env=PRISM_INDEX_ADMIN_HELPER={}",
        helper.display()
    );
}
