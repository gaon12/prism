"""Prism-owned, loopback-only PaddleOCR-VL-1.6 worker.

The parent verifies the interpreter, dependencies, layout model and VLM before
launching this file. No source paths, URLs, document text or credentials are
accepted as command arguments. Requests contain already-authorized bytes.
"""
from __future__ import annotations

import argparse
import hashlib
import hmac
import io
import json
import math
import os
import re
import time
import warnings
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlsplit

PIPELINE = "PaddleOCR-VL-1.6"
MAX_INPUT = 64 << 20
MAX_TEXT = 32 << 20
MAX_SOURCE_PIXELS = 64_000_000
MAX_PAGES = 200


class Failure(Exception):
    def __init__(self, code: str):
        self.code = code


def integer(value, low: int, high: int) -> int:
    # Python bool is an int, but is never a valid size or page number here.
    if type(value) is not int or not low <= value <= high:
        raise Failure("invalidInput")
    return value


def parameters(value) -> dict:
    if not isinstance(value, dict) or set(value) != {
        "kind", "pages", "maxSourcePixels", "maxPixels", "maxDimension",
        "maxTextBytes", "durationMs",
    }:
        raise Failure("invalidInput")
    if value["kind"] not in ("image", "pdf"):
        raise Failure("invalidInput")
    pages = value["pages"]
    if (not isinstance(pages, list) or len(pages) > MAX_PAGES
            or any(type(p) is not int or not 1 <= p <= 1_000_000 for p in pages)
            or any(a >= b for a, b in zip(pages, pages[1:]))
            or (value["kind"] == "image" and pages != [1])):
        raise Failure("invalidInput")
    return {
        **value,
        "maxSourcePixels": integer(value["maxSourcePixels"], 1, MAX_SOURCE_PIXELS),
        "maxPixels": integer(value["maxPixels"], 1, 8_000_000),
        "maxDimension": integer(value["maxDimension"], 1, 4096),
        "maxTextBytes": integer(value["maxTextBytes"], 1, MAX_TEXT),
        "durationMs": integer(value["durationMs"], 1, 3_600_000),
    }


def fit(width: float, height: float, edge: int, pixels: int) -> tuple[int, int]:
    if not all(math.isfinite(v) and v > 0 for v in (width, height)):
        raise Failure("corruptDocument")
    scale = min(1.0, edge / max(width, height), math.sqrt(pixels / (width * height)))
    w, h = max(1, math.floor(width * scale)), max(1, math.floor(height * scale))
    if w * h > pixels or max(w, h) > edge:
        raise Failure("limitExceeded")
    return w, h


def checked_text(result, remaining: int) -> str:
    # The official pipeline has already assembled reading order, including its
    # layout analysis, region crops and VLM recognition. Do not resort blocks by
    # their coordinates or replace this with a single whole-page VLM prompt.
    value = result.json
    blocks = value.get("res", {}).get("parsing_res_list")
    if not isinstance(blocks, list) or len(blocks) > 100_000:
        raise Failure("failed")
    text = []
    used = 0
    for block in blocks:
        content = block.get("block_content") if isinstance(block, dict) else None
        if not isinstance(content, str):
            raise Failure("failed")
        used += len(content.encode("utf-8")) + (1 if text else 0)
        if used > remaining:
            raise Failure("limitExceeded")
        text.append(content)
    return "\n".join(text)


def load_pipeline(root: Path):
    # All inference uses pinned local artifacts. Framework download/telemetry
    # paths are disabled before importing any third-party package.
    for key, value in {
        "HF_HUB_OFFLINE": "1", "TRANSFORMERS_OFFLINE": "1",
        "HF_HUB_DISABLE_TELEMETRY": "1", "DO_NOT_TRACK": "1",
        "PADDLE_PDX_DISABLE_MODEL_SOURCE_CHECK": "True",
        "OMP_NUM_THREADS": "4", "MKL_NUM_THREADS": "4",
        "TOKENIZERS_PARALLELISM": "false",
        "HF_HOME": str(root / "cache" / "hf"),
        "MODELSCOPE_CACHE": str(root / "cache" / "modelscope"),
        "PADDLE_PDX_CACHE_HOME": str(root / "cache" / "paddlex"),
        "XDG_CACHE_HOME": str(root / "cache"),
        "TORCH_HOME": str(root / "cache" / "torch"),
        "TORCHINDUCTOR_CACHE_DIR": str(root / "cache" / "inductor"),
    }.items():
        os.environ[key] = value
    temporary = root / "cache" / "tmp"
    temporary.mkdir(parents=True, exist_ok=True)
    for key in ("TEMP", "TMP", "TMPDIR"):
        os.environ[key] = str(temporary)
    import tempfile
    tempfile.tempdir = str(temporary)
    import torch
    from paddleocr import PaddleOCRVL

    torch.set_num_threads(4)
    return PaddleOCRVL(
        pipeline_version="v1.6", engine="transformers", device="cpu",
        engine_config={"dtype": "float32", "trust_remote_code": False},
        layout_detection_model_dir=str(root / "models" / "layout"),
        vl_rec_model_dir=str(root / "models" / "vlm"),
        use_layout_detection=True, use_doc_orientation_classify=False,
        use_doc_unwarping=False, use_queues=False,
    )


def recognize(pipeline, data: bytes, options: dict) -> list[dict]:
    import numpy as np
    from PIL import Image, ImageOps, UnidentifiedImageError

    options = parameters(options)
    deadline = time.monotonic() + options["durationMs"] / 1000
    if not data or len(data) > MAX_INPUT:
        raise Failure("limitExceeded" if data else "corruptDocument")
    results = []
    used = 0

    def check():
        if time.monotonic() >= deadline:
            raise Failure("timedOut")

    def predict(number: int, image):
        nonlocal used
        check()
        if image.width * image.height > options["maxSourcePixels"]:
            raise Failure("limitExceeded")
        image = ImageOps.exif_transpose(image)
        dimensions = fit(image.width, image.height, options["maxDimension"], options["maxPixels"])
        if dimensions != image.size:
            image = image.resize(dimensions, Image.Resampling.LANCZOS)
        if image.mode in ("RGBA", "LA") or "transparency" in image.info:
            rgba = image.convert("RGBA")
            background = Image.new("RGBA", rgba.size, "white")
            background.alpha_composite(rgba)
            image = background.convert("RGB")
        else:
            image = image.convert("RGB")
        # PaddleX's ndarray input is BGR, whereas Pillow's decoded image is RGB.
        source = np.asarray(image)[:, :, ::-1].copy()
        output = list(pipeline.predict(
            source, use_layout_detection=True, use_queues=False,
            max_new_tokens=4096, max_pixels=min(options["maxPixels"], 1024 * 1024),
        ))
        check()
        if len(output) != 1:
            raise Failure("failed")
        text = checked_text(output[0], options["maxTextBytes"] - used)
        used += len(text.encode("utf-8"))
        results.append({"number": number, "text": text})

    if options["kind"] == "image":
        try:
            with warnings.catch_warnings():
                warnings.simplefilter("error", Image.DecompressionBombWarning)
                with Image.open(io.BytesIO(data)) as image:
                    if image.width * image.height > options["maxSourcePixels"]:
                        raise Failure("limitExceeded")
                    predict(1, image)
        except (Image.DecompressionBombError, Image.DecompressionBombWarning):
            raise Failure("limitExceeded") from None
        except (UnidentifiedImageError, OSError, ValueError):
            raise Failure("corruptDocument") from None
    else:
        import pypdfium2

        try:
            document = pypdfium2.PdfDocument(data)
        except Exception:
            raise Failure("corruptDocument") from None
        try:
            if options["pages"] and options["pages"][-1] > len(document):
                raise Failure("invalidInput")
            for number in options["pages"]:
                check()
                page = document[number - 1]
                bitmap = None
                try:
                    width, height = page.get_size()
                    w, h = fit(width, height, options["maxDimension"], options["maxPixels"])
                    bitmap = page.render(scale=min(w / width, h / height), fill_color=(255, 255, 255, 255))
                    predict(number, bitmap.to_pil())
                finally:
                    if bitmap is not None:
                        bitmap.close()
                    page.close()
        finally:
            document.close()
    return results


def handler(pipeline, token: str):
    class Handler(BaseHTTPRequestHandler):
        server_version = "PrismOCR/1"
        sys_version = ""

        def log_message(self, *_args):
            pass  # Never log request headers, source bytes or recognized text.

        def reply(self, status, value):
            data = json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
            try:
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(data)))
                self.send_header("Connection", "close")
                self.end_headers()
                self.wfile.write(data)
            except OSError:
                pass  # The parent may cancel, or an expired health poll may close.

        def do_GET(self):
            url = urlsplit(self.path)
            query = parse_qs(url.query)
            if url.path != "/v1/health" or set(query) != {"nonce"} or len(query["nonce"]) != 1:
                return self.reply(404, {"error": "invalidInput"})
            nonce = query["nonce"][0]
            if not re.fullmatch("[a-f0-9]{32}", nonce):
                return self.reply(400, {"error": "invalidInput"})
            proof = hashlib.sha256((token + nonce).encode("ascii")).hexdigest()
            self.reply(200, {"pipeline": PIPELINE, "proof": proof})

        def do_POST(self):
            if self.path != "/v1/recognize":
                return self.reply(404, {"error": "invalidInput"})
            if not hmac.compare_digest(self.headers.get("Authorization", "").encode("utf-8"), ("Bearer " + token).encode("ascii")):
                return self.reply(401, {"error": "invalidInput"})
            try:
                if self.headers.get("Transfer-Encoding") is not None:
                    raise Failure("invalidInput")
                raw_size = self.headers.get("Content-Length", "")
                if not raw_size.isascii() or not raw_size.isdecimal():
                    raise Failure("invalidInput")
                length = integer(int(raw_size), 1, MAX_INPUT)
                raw = self.headers.get("X-Prism-OCR", "")
                if len(raw) > 4096:
                    raise Failure("invalidInput")
                options = parameters(json.loads(raw))
                self.connection.settimeout(30)
                data = self.rfile.read(length)
                if len(data) != length:
                    raise Failure("invalidInput")
                pages = recognize(pipeline, data, options)
                self.reply(200, {"engine": "paddleVl16", "pages": pages})
            except Failure as failure:
                self.reply(422, {"error": failure.code})
            except (ValueError, TypeError, KeyError, OverflowError):
                self.reply(400, {"error": "invalidInput"})
            except (BrokenPipeError, ConnectionResetError):
                pass
            except Exception:
                # Native/library exceptions can contain source text. Only a
                # fixed code crosses IPC; no traceback or original error is sent.
                self.reply(500, {"error": "failed"})
    return Handler


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--port", type=int, required=True)
    args = parser.parse_args()
    token = os.environ.pop("PRISM_OCR_TOKEN", "")
    if not re.fullmatch("[a-f0-9]{64}", token) or not 1024 <= args.port <= 65535 or not args.root.is_absolute():
        raise SystemExit(64)
    root = args.root.resolve(strict=True)
    # Bind before loading large models; a conflicting listener fails immediately.
    class Server(HTTPServer):
        request_queue_size = 4

        def get_request(self):
            connection, address = super().get_request()
            connection.settimeout(5)
            return connection, address

        def handle_error(self, _request, _address):
            pass  # No request or library exception is printed by the server.

    server = Server(("127.0.0.1", args.port), BaseHTTPRequestHandler)
    pipeline = load_pipeline(root)
    server.RequestHandlerClass = handler(pipeline, token)
    server.serve_forever(poll_interval=0.1)


if __name__ == "__main__":
    main()
