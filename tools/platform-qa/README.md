# Platform module checks

This independent crate compiles the production runtime inventory, archive extraction,
embedding process, credential store, document extraction, text repository, password-free
PDF retry registry and path
identity files directly. It avoids the Tauri, WebView
and Lance build requirements so these modules can also be checked in a Linux shell.
It does not replace the full application tests or native desktop QA.

Collections checks include the actual EPUB container/package/spine parser,
bounded local CSS/resource rewriting, unscripted HTML/SVG documents, font
obfuscation metadata and current-source cache admission. They do not render
browser pixels. The separate `npm run test:collections-epub` test exercises the
bundled Windows reader and WebP rendering using [independent EPUB fixtures](../../tests/fixtures/epub/README.md).
This is not evidence of Mac/Linux desktop EPUB rendering or complete reading-system
conformance.

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
and resource errors. The macOS adapter uses actual Vision/Quartz SDK bindings;
Linux OS OCR returns an explicit unavailable result. The
managed full PaddleOCR-VL-1.6 fallback is available for Linux x64; actual WSL
image/PDF, automatic fallback, cancellation and forced-parent-exit tests are
opt-in. These library checks do not establish macOS or Linux desktop UI behavior.

On an actual Mac, run both native checks explicitly:

```sh
cargo test --manifest-path tools/platform-qa/Cargo.toml --locked macos_native_ocr_pixels_pdf_passwords_and_resource_errors -- --ignored --nocapture --test-threads=1
cargo test --manifest-path tools/platform-qa/Cargo.toml --locked macos_quartz_preserves_pdf_crop_rotation_and_preview_without_ocr -- --ignored --nocapture --test-threads=1
```

The first requires native Korean recognition and forces the System engine so a
Paddle fallback cannot conceal a native failure. The second checks actual Quartz
pixels, crop/rotation geometry, invalid pages and cancellation without enabling
OCR or requiring a Vision language. Neither test downloads models. Neither has
been executed on macOS yet. Intel/Apple Silicon checks of the adapter's SDK
bindings do not establish whole-app compilation or native behavior. Windows/Linux
tests of the shared decoder and worker ownership establish only those components;
independent PDFium rendering establishes fixture expectations, not Quartz output.

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

Expensive or OS-dependent checks are opt-in:

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
- `real_paddle_install_recognition_and_cancellation`: Windows x64 or Linux x64;
  set `PRISM_TEST_PADDLE_OFFLINE` to the exact artifact directory in the
  [runtime guide](../ocr-runtime/README.md), or explicitly set
  `PRISM_TEST_PADDLE_NETWORK=1` to download the full package. Runs installation,
  complete inventory verification, real Korean image/PDF recognition, process
  reuse and cancellation. Linux needs `PRISM_TEST_LAUNCHER` pointing to the built
  QA binary. It retains a Windows process handle/Linux pidfd to prove exit.
- `real_paddle_auto_fallback_is_reaped_after_app_parent_sigkill`: Linux x64;
  set `PRISM_TEST_PADDLE_INSTALLED` to the authenticated prepared runtime. The
  test copies it to a temporary profile, recognizes real text through Automatic
  dispatch, and proves the exact worker exits after SIGKILL of its own parent.
  It also checks the actual extractor's mixed PDF page order, blank-page omission,
  AES-256 user/owner passwords, raw fingerprints and original-file preservation.
- `native_store_roundtrip_replacement_isolation_and_delete`: on macOS or Linux,
  set `PRISM_TEST_OS_CREDENTIALS=1` and provide an accessible **disposable** user
  keychain or Secret Service session. Creates a random QA service namespace,
  tests synthetic credential replacement/isolation/deletion, and removes its two
  test entries on exit. Never run this against production credentials. Linux QA
  can use a private `dbus-run-session` with an isolated GNOME Keyring data directory.

Run one opt-in test by its name, followed by `-- --ignored --nocapture`. These flags
are deliberately absent from CI's regular platform checks. A passing compile or
archive check alone does not establish actual macOS runtime/keychain behavior.
