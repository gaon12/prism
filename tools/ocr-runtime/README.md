# Local PaddleOCR runtime

Prism uses the official **PaddleOCR-VL-1.6** pipeline, including
**PP-DocLayoutV3** layout detection, region crops, reading order, VLM recognition
and result assembly. It does not substitute a whole-page language-model prompt.
The current runtime lock is for Windows x64 CPU. Other desktop targets remain
work in progress.

The application downloads only the fixed official artifacts in
`windows-x64.lock.json`: Python 3.13.17, PaddleOCR 3.7.0/PaddleX 3.7.2, the CPU
framework and dependencies, and both model inventories. Archives and installed
files have pinned byte lengths and SHA-256 hashes. The compiled inventory digest
also authenticates the complete installed file list, including this worker.
Users do not install Python, pip or Docker. Wheel files are extracted as data;
package installers, setup scripts and console entry points are never executed.

The worker receives already-authorized image/PDF bytes over its private loopback
endpoint. It accepts no document path or URL. The native parent verifies the
worker's secret-bound health proof before sending bytes, and disables proxies
and redirects for document requests. The worker uses pinned local model folders,
offline framework settings and profile-local cache/temp paths. Errors contain
fixed codes, not library exceptions or recognized text. Python runs with `-I -B`;
extra files in the interpreter tree are rejected before launch.

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

The Rust exact-source harness has a separate opt-in production installation,
real image/PDF recognition and cancellation test. It needs about 3 GiB of free
space for its disposable installed copy. `PRISM_TEST_PADDLE_OFFLINE` must point
to an artifact directory with the exact relative paths in the runtime lock.
The test does not use a substitute model or fake recognition response.

```powershell
cargo test --manifest-path tools/platform-qa/Cargo.toml --locked real_paddle_install_recognition_and_cancellation -- --ignored --nocapture
```

Official references:
[PaddleOCR deployment and API guide](https://www.paddleocr.ai/main/en/version3.x/pipeline_usage/PaddleOCR-VL.html),
[Python Windows releases](https://www.python.org/downloads/windows/),
[VLM files](https://huggingface.co/PaddlePaddle/PaddleOCR-VL-1.6),
[layout model files](https://huggingface.co/PaddlePaddle/PP-DocLayoutV3_safetensors).
The downloaded distributions carry their own licenses; the VLM's license is
included in its pinned local inventory.
