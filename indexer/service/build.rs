fn main() {
    println!("cargo:rerun-if-env-changed=PRISM_INDEX_SERVICE_PAYLOAD");
    if std::env::var_os("CARGO_FEATURE_ADMIN").is_none()
        || std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows")
    {
        return;
    }
    let payload = std::env::var_os("PRISM_INDEX_SERVICE_PAYLOAD")
        .expect("Build the real prism-file-index service first and set PRISM_INDEX_SERVICE_PAYLOAD to its absolute path.");
    let payload = std::fs::canonicalize(payload).expect("Real service payload must exist.");
    let bytes = std::fs::read(&payload).expect("Read service payload.");
    assert!(
        bytes.len() >= 64 && bytes.len() <= 16 * 1024 * 1024 && bytes.starts_with(b"MZ"),
        "Service payload must be a bounded Windows executable."
    );
    println!("cargo:rerun-if-changed={}", payload.display());
    println!(
        "cargo:rustc-env=PRISM_INDEX_SERVICE_PAYLOAD={}",
        payload.display()
    );
}
