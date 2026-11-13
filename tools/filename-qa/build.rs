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

    // Keep both sides of a historical comparison on their actual watch loop,
    // including key/seen ordering. Do not maintain a benchmark imitation.
    let index_path = "../../src-tauri/src/index.rs";
    println!("cargo:rerun-if-changed={index_path}");
    let index = fs::read_to_string(index_path).expect("production filename index");
    let anchor = "// Keep watch updates limited to their own subtrees,";
    assert_eq!(index.matches(anchor).count(), 1, "unique watch loop");
    let tail = &index[index.find(anchor).unwrap()..];
    let start = tail
        .find("let mut key = String::new();")
        .expect("watch body");
    let boundary = tail
        .find("\n            if catalog.entries.len() != before")
        .expect("watch body boundary");
    let end = tail[..boundary].rfind('}').expect("watch branch end");
    assert_eq!(tail[end..boundary].trim(), "}");
    let body = &tail[start..end];
    assert!(body.contains("for key in stale {") && body.contains("catalog.entries.remove(&key);"));
    let generated = format!(
        "fn production_update(catalog: &mut WatchCatalog, paths: &[PathBuf], sweep: u64, uncertain: &HashSet<String>) {{\n{body}\n}}\n"
    );
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("watch.rs"),
        generated,
    )
    .expect("write unchanged production watch loop");
}
