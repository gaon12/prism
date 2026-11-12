# Platform module checks

This independent crate compiles the production runtime inventory, archive extraction,
embedding process, credential store, document extraction, text repository, password-free
PDF retry registry and path
identity files directly. It avoids the Tauri, WebView
and Lance build requirements so these modules can also be checked in a Linux shell.
It does not replace the full application tests or native desktop QA.

Existing-form checks import the production DOCX/HWPX codec directly. Run
`cargo test --manifest-path tools/platform-qa/Cargo.toml --locked form_fill`.
They cover split runs, repeated fields, independent named/positional keys,
value/output limits, changed originals, hard links and atomic new-file saves.
These are package/XML and filesystem checks; they do not prove native
Word/Hancom layout or the desktop form editor's navigation behavior.

Power checks import the exact production OS adapter and test Windows flags,
macOS `pmset` parsing, Linux power-supply rules and the fixed command's output
and timeout limits. Run `cargo test --manifest-path tools/platform-qa/Cargo.toml
--locked power::tests`. Its ignored subprocess fixture is invoked by the parent
test. These checks do not change the machine's power state or prove physical
charger transitions. Full-app content-service tests separately inject power
readings to verify active-scan cancellation, retained search data, AC resumption
and preservation of manual pause; they are not included in this smaller crate.

Generated-document PDF checks compile the exact production Markdown converter,
closed in-memory Typst World, export-admission registry and atomic file writer.
They typeset actual multilingual/emoji text, tables, code, repeated footnotes and
multiple pages, then parse the resulting PDF; no successful PDF response is
fabricated. They also verify denied file access, literal HTML/code, omitted
network images, early cancellation, retained worker admission and replacement of
an actual temporary destination. Fonts are the application's pinned bundled
assets. Run `cargo test --manifest-path tools/platform-qa/Cargo.toml --locked document_pdf`
for these checks. They do not exercise the native save picker or prove desktop
integration on an untested OS.

For visual review, set `PRISM_TEST_DOCUMENT_PDF_CAPTURE_DIR` to a newly created
empty directory before these tests. The actual single-page and five-page outputs
are written as `multilingual.pdf` and `multipage.pdf` using create-new semantics;
an existing artifact is never replaced. Inspect them with an independent PDF
renderer as well as the text assertions.

Collections checks include the actual EPUB container/package/spine parser,
bounded local CSS/resource rewriting, unscripted HTML/SVG documents, font
obfuscation metadata and current-source cache admission. They do not render
browser pixels. The separate `npm run test:collections-epub` test exercises the
bundled Windows reader and WebP rendering using [independent EPUB fixtures](../../tests/fixtures/epub/README.md).
This is not evidence of Mac/Linux desktop EPUB rendering or complete reading-system
conformance.

On Linux, the exact production `pdf_renderer.rs` runs Hayro 0.9.0 in an owned
pre-GUI subprocess. Ordinary tests verify actual crop/rotation colors, typed,
scanned and blank pages, in-memory decrypted inputs with both public password
roles, and collection import/first-page WebP/reader/cache/source-change behavior
with OCR still disabled. Process checks retain a pidfd to prove that cancellation
and actual parent SIGKILL terminate the owned worker while a sibling survives.
An oversized pipe request is rejected before payload allocation. No Python,
model, system PDF renderer or synthetic successful render response is used.

```sh
cargo test --manifest-path tools/platform-qa/Cargo.toml --locked --test pdf_renderer -- --nocapture
```

Set `PRISM_TEST_PDF_CAPTURE_DIR` to a new empty directory to retain the actual
PNG/WebP outputs for visual review; files are created without overwriting.
These are Linux library/owned-process checks, not a full Tauri desktop/AppImage
test. ARM compilation/execution, missing custom-font substitution and complete
PDF feature coverage remain unverified. The production launcher is always
`current_exe`; the QA host implements the same dispatch before any other work.

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
OCR or requiring a Vision language. Neither test downloads models. Both passed
as explicit opt-in runs on macOS 26.7.2 arm64 in
[CI job 128564691627](https://github.com/gaon12/prism/actions/runs/42426131901/job/128564691627)
at `ce6715592b1a69773f9dd6adc6a4e4ec00597d47`: Quartz in 0.04s and Korean
Vision in 8.04s. The ordinary suite still reports them as ignored; the separate
steps execute them. This verifies those native library fixtures, not desktop UI,
Intel runtime behavior, managed Paddle/Office packages or arbitrary documents.
Cross-target SDK checks and independent PDFium rendering remain separate from
actual Vision/Quartz execution.

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

  Run only this opt-in check with the following PowerShell commands (use an
  absolute archive directory; temporary extraction can use another local drive):

  ```powershell
  $env:PRISM_TEST_PACKAGES_DIR = 'D:\runtime-archives'
  cargo test --manifest-path tools/platform-qa/Cargo.toml --locked actual_upstream_packages_extract_verify_and_materialize_all_aliases -- --ignored --nocapture --test-threads=1
  ```

  On 2026-11-08, the exact production inventory, extractor and external test
  module passed this check on Windows using all five pinned b11960 archives:
  Windows x64, macOS ARM64/x64 and Linux ARM64/x64. Extraction and subsequent
  installed-byte verification passed, including regular-file materialization of
  library aliases. The archive-only harness used all 51 dependencies at the
  application's locked versions. This establishes archive compatibility on the
  Windows host, not execution, model inference or native installation on the
  other operating systems.

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
- `real_paddle_install_recognition_and_cancellation`: Windows x64, Linux x64 or
  staged Mac ARM64 debug/test qualification;
  set `PRISM_TEST_PADDLE_OFFLINE` to the exact artifact directory in the
  [runtime guide](../ocr-runtime/README.md), or explicitly set
  `PRISM_TEST_PADDLE_NETWORK=1` to download the full package. Runs installation,
  complete inventory verification, real Korean image/PDF recognition, process
  reuse and cancellation. Unix needs `PRISM_TEST_LAUNCHER` pointing to the built
  QA binary. Mac also needs `PRISM_DEV_PADDLE_REFERENCE=1`; release Mac runtime
  availability is unchanged. It retains a Windows process handle, Linux pidfd or
  Mac kernel exit notification to prove exit. The explicit Mac runs
  `43180553078` and `43186709347` passed this installation/recognition/cancellation
  check. Their separate parent-lifetime check failed before SIGKILL and remains
  unqualified.
- `real_paddle_auto_fallback_is_reaped_after_app_parent_sigkill`: Linux x64 or
  staged Mac ARM64 debug/test qualification;
  set `PRISM_TEST_PADDLE_INSTALLED` to the authenticated prepared runtime. The
  test copies it to a temporary profile, recognizes real text through Automatic
  dispatch, and proves the exact worker exits after SIGKILL of its own parent.
  It also checks the actual extractor's mixed PDF page order, blank-page omission,
  AES-256 user/owner passwords, raw fingerprints and original-file preservation.
  Mac requires the explicit reference flag and an actual materialized Mac seed,
  then exercises real OS language unavailability before Auto fallback. The worker
  exit observer attaches before parent termination. Each fixed progress stage has
  the production 180-second OCR allowance, and the complete fixture has a
  900-second ceiling for its four independent OCR/extraction calls plus setup.
  Missing, repeated, unknown or backward phases cannot extend a stalled stage.
  On Mac run `43186709347`, the image completed in 112.439s and mixed extraction
  in 62.784s; the old aggregate 180-second bound killed the next request at
  `encrypted-user-extraction`. This measured result motivates separate operation
  budgets; it does not qualify the remaining extraction or SIGKILL assertions.
  See the [runtime guide](../ocr-runtime/README.md)
  for exact native commands. Compilation does not establish native execution.
- `native_store_roundtrip_replacement_isolation_and_delete`: on macOS or Linux,
  set `PRISM_TEST_OS_CREDENTIALS=1` and provide an accessible **disposable** user
  keychain or Secret Service session. Creates a random QA service namespace,
  tests synthetic credential replacement/isolation/deletion, and removes its two
  test entries on exit. Never run this against production credentials. Linux QA
  can use a private `dbus-run-session` with an isolated GNOME Keyring data directory.

Run one opt-in test by its name, followed by `-- --ignored --nocapture`. These flags
are absent from CI's regular platform checks. The macOS matrix separately selects
the native Quartz PDF and Korean Vision tests explicitly; other model/credential
opt-ins still require their stated environment. Inspect those specific job steps
before claiming native execution. A passing compile or archive check alone does
not establish actual macOS runtime/keychain behavior.
