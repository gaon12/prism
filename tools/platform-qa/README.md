# Platform module checks

This independent crate compiles the production runtime inventory, archive extraction,
embedding process and credential store files directly. It avoids the Tauri, WebView
and Lance build requirements so these modules can also be checked in a Linux shell.
It does not replace the full application tests or native desktop QA.

```sh
cargo test --manifest-path tools/platform-qa/Cargo.toml --locked
cargo clippy --manifest-path tools/platform-qa/Cargo.toml --locked --all-targets -- -D warnings
```

Ordinary tests use synthetic archives and temporary application directories. Windows
tests additionally use DPAPI and privately owned child processes. macOS and Linux
ordinary tests do not access the system credential store.

Three expensive or OS-dependent checks are opt-in:

- `actual_upstream_packages_extract_verify_and_materialize_all_aliases`: set
  `PRISM_TEST_PACKAGES_DIR` to a directory containing all five archives named in
  `src-tauri/src/embedding_assets.rs`. Checks their pinned lengths and SHA-256 hashes,
  extracts them to temporary directories, and verifies every installed file. Does
  not execute foreign-platform binaries.
- `real_korean_retrieval_and_owned_process_shutdown`: set `PRISM_TEST_MODEL` to the
  pinned Q8 model and `PRISM_TEST_RUNTIME` to the **host platform's** pinned archive.
  Runs actual Korean retrieval at 768 and 256 dimensions, reopens installed state,
  and checks normal runtime cleanup. This does not test forced parent termination
  on Unix, where the Windows Job Object mechanism is unavailable.
- `native_store_roundtrip_replacement_isolation_and_delete`: on macOS or Linux,
  set `PRISM_TEST_OS_CREDENTIALS=1` and provide an accessible **disposable** user
  keychain or Secret Service session. Creates a random QA service namespace,
  tests synthetic credential replacement/isolation/deletion, and removes its two
  test entries on exit. Never run this against production credentials. Linux QA
  can use a private `dbus-run-session` with an isolated GNOME Keyring data directory.

Run one opt-in test by its name, followed by `-- --ignored --nocapture`. These flags
are deliberately absent from CI's regular platform checks. A passing compile or
archive check alone does not establish actual macOS runtime/keychain behavior.
