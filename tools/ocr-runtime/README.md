# Local PaddleOCR runtime

Prism uses the official **PaddleOCR-VL-1.6** pipeline, including
**PP-DocLayoutV3** layout detection, region crops, reading order, VLM recognition
and result assembly. It does not substitute a whole-page language-model prompt.
Runtime locks cover Windows x64, Linux x64 and macOS 14+ ARM64 CPU. Intel macOS
and Linux ARM64 remain unavailable; full desktop integration is separate from
the native runtime and WSL checks.

The application downloads only the fixed official artifacts in
`windows-x64.lock.json`, `linux-x64.lock.json` and the unchanged
`macos-arm64.reference.lock.json`: private Python,
PaddleOCR 3.7.0/PaddleX 3.7.2, the CPU
framework and dependencies, and both model inventories. Archives and installed
files have pinned byte lengths and SHA-256 hashes. The compiled inventory digest
also authenticates the complete installed file list, including this worker.
Windows uses CPython 3.13.17
from python.org; Linux uses the pinned 3.13.18 install-only archive from
[Astral Python Build Standalone](https://github.com/astral-sh/python-build-standalone/releases/tag/20261026).
Users do not install Python, pip or Docker. Wheel files are extracted as data;
package installers, setup scripts and console entry points are never executed.

The worker receives already-authorized image/PDF bytes over its private loopback
endpoint. It accepts no document path or URL. The native parent verifies the
worker's secret-bound health proof before sending bytes, and disables proxies
and redirects for document requests. The worker uses pinned local model folders,
offline framework settings and profile-local cache/temp paths. Errors contain
fixed codes, not library exceptions or recognized text. Python runs with `-I -B`;
extra files in the interpreter tree are rejected before launch. Linux also uses
`-S` and its authenticated bootstrap to add only the private wheel directory,
without executing site `.pth` files. Internal interpreter archive aliases become
regular copies; no filesystem links or setuid archive permissions are installed.

The application owns the child through its normal Windows Job Object/Unix
supervisor implementation. Cancelling active recognition kills that owned
worker; idle workers are released after two minutes. Model installation is
explicit and does not enable OCR, expand folder permissions or resume a paused
index. OS OCR remains the default first choice in Automatic mode.

## Maintainer workflow

This is a development workflow, not something users must run. Obtain the
official Python 3.13.17 embeddable x64 archive, then download wheels:

```powershell
python -m pip download --only-binary=:all: --python-version 3.13 --implementation cp --abi cp313 --platform win_amd64 --dest <wheelhouse> -r tools/ocr-runtime/requirements-windows-x64.txt
```

Obtain the model files from these immutable official revisions:

- `PaddlePaddle/PaddleOCR-VL-1.6`:
  `c5630abae1d940eafe0697512a0325494b02ab42`
- `PaddlePaddle/PP-DocLayoutV3_safetensors`:
  `e4f489dc536556fc1be02b973ba06756e0d4a2ac`

`lock_assets.py` checks wheel hashes against official PyPI/PyTorch index metadata,
checks the interpreter pin and model manifest, and derives the installed
inventory digest. Its `--materialize` option only creates a **new** QA directory.
It never executes a downloaded package. Model-manifest entries use
`path` (`layout/...` or `vlm/...`), `size`, `sha256` and an immutable `url`.

```powershell
python tools/ocr-runtime/lock_assets.py --wheelhouse <wheelhouse> --python-archive <python-3.13.17-embed-amd64.zip> --model-manifest <model-files.json> --model-directory <models> --output tools/ocr-runtime/windows-x64.lock.json --materialize <new-QA-folder>
<new-QA-folder>/python/python.exe -I -B -m unittest discover -s tools/ocr-runtime/tests -v
```

For Linux x64, use a disposable maintainer interpreter to download wheels from
`requirements-linux-x64.txt` on the target platform. Pass `--target linux-x64`
and the pinned `cpython-3.13.18+20261026-x86_64-unknown-linux-gnu-install_only_stripped.tar.gz`
archive to the same builder. Generate on a case-sensitive filesystem; the Unix
stdlib includes distinct case-sensitive names. The builder verifies the upstream
archive SHA-256, resolves only internal aliases, and hashes each installed copy.
The Linux package currently downloads 2,525,472,054 bytes and expands to
3,704,586,012 bytes in 38,709 files. Upstream Linux wheels still require compatible
OS libraries; the WSL result does not establish all AppImage distributions.

### macOS ARM64 preparation

`macos-arm64.sources.lock.json` pins official sources for macOS 14+ and Python
3.13.18: the standalone interpreter, 107 compatible wheels and the same 21 model
files. Dependency metadata was checked against the target's Python, platform and
requested extras. The macOS PyTorch wheels use versions without Linux's `+cpu`
suffix; `worker.py` still selects CPU and the complete official pipeline.

This is a **source-only plan**, not an installed inventory or a runtime offered
by the application. Actual Mac installation, native imports, recognition,
cancellation and package verification remain required. Windows cannot establish
those results by inspecting Mach-O files. The ARM64 plan does not qualify Intel
execution or change the Windows/Linux locks.

Prepare the exact source-lock artifacts in a **new** maintainer directory:

```sh
python -B tools/ocr-runtime/prepare_sources.py --output /path/to/new-mac-sources
```

The helper streams each source under its size bound and verifies its full SHA
before publication. Existing output folders are refused. Failed partials and a
sanitized `status.json` remain for diagnosis; `model-files.json` is published only
after every source passes. Status always records `actualExecution=false` and a
canonical plan hash. It does not parse archives, install anything or run Python
from the downloaded interpreter. Do not upload its whole source directory as
test evidence.

Optional `--reuse-models /path/to/existing/models` makes independent, fully
verified copies of all 21 model files instead of downloading them again. A
missing or incorrect cache fails preparation; it does not silently fall back to
another source or link writable staged files to an existing runtime.

On the actual Mac, generate a new inventory using the prepared inputs:

```sh
python -B tools/ocr-runtime/lock_assets.py --target macos-arm64 --wheelhouse /path/to/new-mac-sources/wheels --python-archive /path/to/new-mac-sources/cpython-3.13.18+20261026-aarch64-apple-darwin-install_only_stripped.tar.gz --model-manifest /path/to/new-mac-sources/model-files.json --model-directory /path/to/new-mac-sources/models --output /path/to/new-macos-arm64.lock.json --materialize /path/to/new-private-runtime
```

The resulting lock still needs native verification before release registration
in the app. Users never run these tools or install Python themselves.

`macos-arm64.inventory-reference.json` records the data-only reference generated
from all 129 authenticated inputs: 36,322 expected installed files, 3,256,648,939
expanded bytes and the inventory/catalog hashes. It is not a runtime catalog and
is not itself the app's runtime catalog. Compare an actual Mac build against these fields
before native qualification. Windows archive parsing does not establish Mac
filesystem, import, recognition, cancellation or process-lifetime behavior.

`macos-arm64.reference.lock.json` is that complete, byte-pinned reference catalog.
After the successful [native qualification](https://github.com/gaon12/prism/actions/runs/43297322002),
the app selects this same catalog on macOS 14+ ARM64 in debug and release builds,
without `PRISM_DEV_PADDLE_REFERENCE`. Intel and older macOS versions remain
unavailable. The source plan, reference catalog and worker are checked against
their compiled hashes. The historical `inventory-reference.json` still describes
its original data-only generation; `macos-arm64.qualification.json` records the
subsequent native evidence, exact catalog and artifact hashes. Ordinary tests do
not download or execute this runtime.

The ignored Rust check
`actual_macos_reference_extracts_and_authenticates_every_installed_file` can
verify the complete reference as **data** on a maintainer machine. Set
`PRISM_TEST_MAC_PADDLE_ARTIFACTS` to the prepared 129-artifact directory and
`PRISM_TEST_MAC_PADDLE_DATA_OUTPUT` to a new absolute output directory whose
parent already exists. Existing outputs are refused; both successful and failed
outputs are retained. Allow at least 3.3 GB for its independent installed copy.

```sh
cargo test --manifest-path tools/platform-qa/Cargo.toml --locked actual_macos_reference_extracts_and_authenticates_every_installed_file -- --ignored --nocapture --test-threads=1
```

The check authenticates every input, uses the real Rust archive installers,
validates the full installed inventory, reopens it and rejects an unexpected
importable file. It preserves the verified runtime and a separate `result.json`.
No downloaded interpreter, worker or model runs. A Windows result proves its
data extraction and byte checks, not Mac filesystem, imports or inference.

On an actual macOS 14+ ARM64 machine with the prepared, authenticated artifacts,
run the complete production installer and image/PDF/cancellation test:

```sh
export PRISM_TEST_PADDLE_OFFLINE=/absolute/path/to/new-mac-sources
cargo build --manifest-path tools/platform-qa/Cargo.toml --locked --bin prism-platform-qa
export PRISM_TEST_LAUNCHER="$PWD/tools/platform-qa/target/debug/prism-platform-qa"
cargo test --manifest-path tools/platform-qa/Cargo.toml --locked real_paddle_install_recognition_and_cancellation -- --ignored --nocapture
```

If `CARGO_TARGET_DIR` is set, use that directory's `debug/prism-platform-qa`
instead. The test installs a disposable private runtime, rejects an injected
unexpected file, executes the full official pipeline on the real bilingual PNG
and mixed PDF, checks process reuse and cancels active recognition. It attaches
a kernel `NOTE_EXIT` notification while the owned worker is alive and waits on
that retained queue after cancellation; it does not infer exit from a later PID
lookup. A separate small native test exercises this observer using its own
`/bin/sleep` child. Cross-compilation alone does not establish either result.
The [Mac ARM64 run](https://github.com/gaon12/prism/actions/runs/43297322002)
matched the unchanged reference catalog byte for byte and passed actual
installation, image/PDF recognition, process reuse and cancellation (196.07s).
Its separate Automatic fallback, mixed/encrypted extraction and app-parent
SIGKILL/worker-exit test also passed (250.49s), retaining the eight-second worker
exit deadline. The host was macOS 26.7.2 ARM64. This qualifies the native runtime;
it does not establish desktop installation UI or every supported OS version.
The parent fixture records fixed stage names and elapsed times in its own log
and beside its readiness file. On failure, the test emits the last phase and at
most 16 KiB from the end of `fixture.log` after cleaning up its owned parent and
before removing temporary files. The existing `parent-lifetime.log` capture
retains these diagnostics without an extra environment variable or a runtime
directory upload. This diagnostic does not extend the deadline or skip
extraction and original-file checks. Windows reference generation cannot replace
these native checks.

The Rust exact-source harness has a separate opt-in production installation,
real image/PDF recognition and cancellation test. It needs about 3 GiB of free
space for its disposable installed copy. `PRISM_TEST_PADDLE_OFFLINE` must point
to an artifact directory with the exact relative paths in the runtime lock.
The test does not use a substitute model or fake recognition response. On Linux,
first build the QA binary and set `PRISM_TEST_LAUNCHER` to its absolute path so
the actual Unix supervisor can dispatch before application startup. For an explicit
full HTTPS installation test, set `PRISM_TEST_PADDLE_NETWORK=1`; it downloads the
entire package instead of reading the offline directory.

```powershell
cargo test --manifest-path tools/platform-qa/Cargo.toml --locked real_paddle_install_recognition_and_cancellation -- --ignored --nocapture
```

The Linux x64 and staged Mac ARM64 opt-in
`real_paddle_auto_fallback_is_reaped_after_app_parent_sigkill` uses
`PRISM_TEST_PADDLE_INSTALLED` as an authenticated runtime seed. It recognizes the
actual Korean image through public Automatic dispatch and sends SIGKILL only to
its own app-parent fixture. Linux polls the retained worker pidfd; Mac waits on
the kernel exit notification registered before killing that parent. The normal
production supervisor must terminate the same owned worker within eight seconds.
This uses real inference, not an OCR substitute. On Mac, a valid unsupported
language first has to produce a real OS `LanguageUnavailable` error; only then
may Automatic fall back to the installed full Paddle pipeline.

The same fixture exercises document extraction for mixed typed/scanned pages,
blank-page omission and AES-256 user/owner passwords, retaining original source
hashes and bytes without writing a decrypted PDF. On macOS 14+ ARM64, use the
actual materialized Mac runtime as the seed; no reference flag is needed:

```sh
export PRISM_TEST_PADDLE_INSTALLED=/absolute/path/to/new-private-runtime
cargo test --manifest-path tools/platform-qa/Cargo.toml --locked --test paddle_lifetime real_paddle_auto_fallback_is_reaped_after_app_parent_sigkill -- --ignored --nocapture
```

This separate test builds its own QA host and does not require
`PRISM_TEST_LAUNCHER`. A compilation result does not qualify the actual model,
source extraction or parent-death behavior. The linked native run supplies that
evidence for the unchanged catalog; future runtime changes need new qualification.

For the real Windows WebView regression, set `PRISM_TEST_PADDLE_RUNTIME` to the
materialized QA directory and run `npm run test:paddle-ocr`. It clicks the actual
download/cancel controls first, then copies that fully authenticated runtime into
its isolated profile for restart and real content-index recognition. It checks
manual selection, automatic fallback for an unavailable OS language, filename-only
scope exclusion, retained text after disabling OCR and settings stability. The
runtime data setup does not replace inference or native responses. The offline
folder picker itself is not automated by this test.

Official references:
[PaddleOCR deployment and API guide](https://www.paddleocr.ai/main/en/version3.x/pipeline_usage/PaddleOCR-VL.html),
[Python Windows releases](https://www.python.org/downloads/windows/),
[VLM files](https://huggingface.co/PaddlePaddle/PaddleOCR-VL-1.6),
[layout model files](https://huggingface.co/PaddlePaddle/PP-DocLayoutV3_safetensors).
The downloaded distributions carry their own licenses; the VLM's license is
included in its pinned local inventory.
