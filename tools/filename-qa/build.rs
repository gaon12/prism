use std::{env, fs, path::PathBuf};

fn main() {
    // The mapper is private to filename_service::catalog. Emit its unchanged
    // production source into the benchmark's index scope; do not reimplement it.
    let path = "../../src-tauri/src/filename_catalog.rs";
    println!("cargo:rerun-if-changed={path}");
    let source = fs::read_to_string(path).expect("production filename catalog");
    let start = source
        .find("pub(super) fn raw(record: FileRecord) -> Raw {")
        .expect("raw mapper");
    let tail = &source[start..];
    let end = tail
        .find("\npub(super) fn cancelled()")
        .expect("raw mapper boundary");
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("raw.rs"),
        &tail[..end],
    )
    .expect("write unchanged raw mapper");
}
