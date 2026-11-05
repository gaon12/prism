# Platform module checks

This independent crate compiles the production runtime inventory, archive extraction,
embedding process, credential store, document extraction, text repository, password-free
PDF retry registry and path
identity files directly. It avoids the Tauri, WebView
and Lance build requirements so these modules can also be checked in a Linux shell.
It does not replace the full application tests or native desktop QA.

```sh
cargo test --manifest-path tools/platform-qa/Cargo.toml --locked
cargo clippy --manifest-path tools/platform-qa/Cargo.toml --locked --all-targets -- -D warnings
```

Ordinary tests use synthetic archives and temporary application directories. Windows
tests additionally use DPAPI and privately owned child processes. Unix tests start
inert localhost server fixtures through the production supervisor, exercise normal
stop and actual SIGKILL of their owned parent, and check that a sibling survives.
An unresponsive-launcher check also exercises the ten-second handshake timeout
and two-second shutdown grace period, then verifies private-group cleanup. It
never signals a cached process ID after its child may have been reaped.
macOS and Linux ordinary tests do not access the system credential store.

Password-PDF tests use independent pypdf-generated RC4-40/128 and AES-128/256
fixtures with public test passwords. They check both password roles, wrong
passwords, raw source fingerprints, original page numbers and extraction budgets.
Registry tests cover early cancellation, replay, window isolation and parser-slot
lifetimes. The same locally patched PDF dependencies are used by the app and this
crate; upstream provenance and the narrow legacy owner-key correction are recorded
under `src-tauri/vendor/`. This crate does not exercise the app's retry dialog or
embedding of unlocked text; use the native PDF test for that flow.

The shared OCR module validates opt-in requests, cancellation, admission leases and
pixel scaling without requiring an OS engine in ordinary tests. On Windows,
`windows_native_ocr_pixels_pdf_passwords_and_resource_errors -- --ignored --nocapture`
explicitly exercises the installed Korean OCR engine against the independent
[pixel/PDF fixtures](../../tests/fixtures/ocr/README.md). It checks transparent
images, original mixed-page order, AES-256 user/owner passwords, missing languages
and resource errors. Non-Windows OCR adapters remain unimplemented and return
an explicit unavailable result; these platform checks do not establish native
Mac/Linux recognition.

The [native path policy workflow](../../.github/workflows/path-policy.yml) runs
these library tests on macOS with `TMPDIR=/var/tmp`. It intentionally exercises
Apple's real `/var` alias rather than replacing it with a plain runner directory.
Dedicated tests create owned fixtures explicitly under `/tmp`, check all three
system aliases, reject nested user symlinks, and verify extraction, scoped
keyword search and deletion of cached metadata. The exception requires exact
root aliases, root ownership, their expected `/private` targets and protected
parent directories. It never authorizes an arbitrary selected symlink.
Cross-target compilation alone does not establish these runtime results.
The harness locally allows the imported modules' existing style lint baseline
and application-only dead code; the full application still reports those lints.

Four expensive or OS-dependent checks are opt-in:

- `actual_upstream_packages_extract_verify_and_materialize_all_aliases`: set
  `PRISM_TEST_PACKAGES_DIR` to a directory containing all five archives named in
  `src-tauri/src/embedding_assets.rs`. Checks their pinned lengths and SHA-256 hashes,
  extracts them to temporary directories, and verifies every installed file. Does
  not execute foreign-platform binaries.
- `real_korean_retrieval_and_owned_process_shutdown`: set `PRISM_TEST_MODEL` to the
  pinned Q8 model and `PRISM_TEST_RUNTIME` to the **host platform's** pinned archive.
  Runs actual Korean retrieval at 768 and 256 dimensions, reopens installed state,
  and checks normal runtime cleanup. On Unix, first build this QA binary with
  `cargo build --manifest-path tools/platform-qa/Cargo.toml --locked`, then set
  `PRISM_TEST_SUPERVISOR` to its absolute executable path. This supplies the same
  pre-GUI supervisor dispatcher used by the application; Rust's test-runner binary
  does not have that entry point. Windows does not need this variable.
- `real_embedding_server_is_reaped_after_the_app_parent_is_killed`: on Unix, set
  the same pinned model/runtime variables. The QA fixture uses the actual
  `EmbeddingRuntime`, imports the files, starts the server and embeds a Korean
  query. The test sends SIGKILL to its own app-parent fixture and checks that the
  actual server is gone. This uses the normal production supervisor, not a model
  substitute. It does not require `PRISM_TEST_SUPERVISOR` because the parent is
  already the QA binary with the proper dispatcher.
- `native_store_roundtrip_replacement_isolation_and_delete`: on macOS or Linux,
  set `PRISM_TEST_OS_CREDENTIALS=1` and provide an accessible **disposable** user
  keychain or Secret Service session. Creates a random QA service namespace,
  tests synthetic credential replacement/isolation/deletion, and removes its two
  test entries on exit. Never run this against production credentials. Linux QA
  can use a private `dbus-run-session` with an isolated GNOME Keyring data directory.

Run one opt-in test by its name, followed by `-- --ignored --nocapture`. These flags
are deliberately absent from CI's regular platform checks. A passing compile or
archive check alone does not establish actual macOS runtime/keychain behavior.
