"""Prepare pinned maintainer inputs as data; never install or execute packages."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import time
import urllib.parse
import urllib.request
import uuid

SOURCE_LOCK = Path(__file__).with_name("macos-arm64.sources.lock.json")
CHUNK = 1 << 19


class PreparationError(Exception):
    def __init__(self, code):
        self.code = code
        super().__init__(f"Runtime source preparation failed ({code})")


def https(url):
    try:
        parsed = urllib.parse.urlsplit(url)
        return (parsed.scheme == "https" and bool(parsed.hostname)
                and not parsed.username and not parsed.password
                and parsed.port in (None, 443) and not parsed.fragment)
    except (ValueError, TypeError):
        return False


class HttpsRedirects(urllib.request.HTTPRedirectHandler):
    max_redirections = 5

    def redirect_request(self, request, fp, code, message, headers, newurl):
        if not https(newurl):
            raise PreparationError("redirect")
        return super().redirect_request(request, fp, code, message, headers, newurl)


def validate(plan):
    if (not isinstance(plan, dict) or plan.get("schema") != 1
            or plan.get("sourceOnly") is not True or plan.get("target") != "macos-arm64"
            or plan.get("pipeline") != "PaddleOCR-VL-1.6"
            or plan.get("pythonVersion") != "3.13.18" or plan.get("minimumMacOS") != "14.0"
            or not isinstance(plan.get("assets"), list) or not 1 <= len(plan["assets"]) <= 256):
        raise PreparationError("manifest")
    seen = set()
    for asset in plan["assets"]:
        if not isinstance(asset, dict):
            raise PreparationError("manifest")
        path = asset.get("path")
        if (not isinstance(path, str) or len(path) > 2048
                or re.fullmatch(r"[A-Za-z0-9_+./-]+", path) is None
                or any(p in ("", ".", "..") or p.endswith(".") for p in path.split("/"))
                or path.casefold() in seen):
            raise PreparationError("manifest")
        seen.add(path.casefold())
        parts = path.split("/")
        kind = asset.get("kind")
        if not ((kind == "pythonTarGz" and len(parts) == 1 and path.endswith(".tar.gz"))
                or (kind == "wheel" and len(parts) == 2 and parts[0] == "wheels" and path.endswith(".whl"))
                or (kind == "model" and len(parts) >= 3 and parts[0] == "models")):
            raise PreparationError("manifest")
        if (type(asset.get("bytes")) is not int or not 1 <= asset["bytes"] <= 2 << 30
                or not isinstance(asset.get("sha256"), str)
                or re.fullmatch(r"[0-9a-f]{64}", asset["sha256"]) is None
                or not https(asset.get("url"))
                or urllib.parse.urlsplit(asset["url"]).hostname not in {
                    "github.com", "files.pythonhosted.org", "huggingface.co",
                }):
            raise PreparationError("manifest")
    kinds = [asset["kind"] for asset in plan["assets"]]
    if kinds.count("pythonTarGz") != 1 or "wheel" not in kinds or "model" not in kinds:
        raise PreparationError("manifest")


def transfer(asset, directory, source, deadline, clock):
    partial = directory / (".partial-" + uuid.uuid4().hex)
    target = directory / asset["path"]
    target.parent.mkdir(parents=True, exist_ok=True)
    sha = hashlib.sha256()
    copied = 0
    with partial.open("xb") as output:
        while True:
            if clock() >= deadline:
                raise PreparationError("timeout")
            block = source.read(CHUNK)
            if not block:
                break
            copied += len(block)
            if copied > asset["bytes"]:
                raise PreparationError("size")
            output.write(block)
            sha.update(block)
    if copied != asset["bytes"]:
        raise PreparationError("size")
    if sha.hexdigest() != asset["sha256"]:
        raise PreparationError("hash")
    # Publish on this same volume without replacing an existing destination.
    # This links only our verified partial, never a cached source model.
    os.link(partial, target)
    partial.unlink()


def prepare(plan, output, *, reuse_models=None, opener=None, timeout=300, clock=time.monotonic):
    """Plan/transport injection is for inert helper tests; CLI pins are fixed."""
    validate(plan)
    if type(timeout) not in (int, float) or not 0 < timeout <= 3600:
        raise PreparationError("arguments")
    # Refuse an existing leaf even if it is a dangling link. Resolving its
    # parents still supports the normal /var -> /private/var alias on macOS.
    if os.path.lexists(output):
        raise PreparationError("directory")
    directory = Path(output).resolve()
    try:
        directory.mkdir(parents=True, exist_ok=False)
    except OSError:
        raise PreparationError("directory") from None
    (directory / ".prism-source-owner").write_text("Prism runtime source preparation v1\n", encoding="utf-8")
    status = {"prepared": False, "actualExecution": False, "target": plan["target"],
              "canonicalPlanSha256": hashlib.sha256(json.dumps(plan, sort_keys=True, separators=(",", ":")).encode()).hexdigest(),
              "verifiedFiles": 0, "expectedFiles": len(plan["assets"]),
              "verifiedBytes": 0, "expectedBytes": sum(a["bytes"] for a in plan["assets"]),
              "phase": "preparing", "current": None}

    def save():
        (directory / "status.json").write_text(json.dumps(status, indent=2) + "\n", encoding="utf-8")

    opener = opener or urllib.request.build_opener(HttpsRedirects())
    try:
        reuse = Path(reuse_models).resolve(strict=True) if reuse_models is not None else None
        for asset in plan["assets"]:
            status["current"] = asset["path"]
            save()
            deadline = clock() + timeout
            if asset["kind"] == "model" and reuse is not None:
                cached = (reuse / Path(*asset["path"].split("/")[1:])).resolve(strict=True)
                if not cached.is_relative_to(reuse) or not cached.is_file():
                    raise PreparationError("reuseSource")
                # Copy and authenticate the bytes; a staged model must never
                # share a writable hard link with an existing installed model.
                with cached.open("rb") as source:
                    transfer(asset, directory, source, deadline, clock)
            else:
                request = urllib.request.Request(asset["url"], headers={
                    "User-Agent": "Prism-runtime-source-preparation/0.1",
                })
                with opener.open(request, timeout=min(20, timeout)) as source:
                    if getattr(source, "status", 200) != 200:
                        raise PreparationError("http")
                    transfer(asset, directory, source, deadline, clock)
            status["verifiedFiles"] += 1
            status["verifiedBytes"] += asset["bytes"]
        model_manifest = {"pipeline": "PaddleOCR-VL-1.6", "files": [
            {"path": a["path"][len("models/"):], "size": a["bytes"],
             "sha256": a["sha256"], "url": a["url"]}
            for a in plan["assets"] if a["kind"] == "model"
        ]}
        with (directory / "model-files.json").open("x", encoding="utf-8") as file:
            json.dump(model_manifest, file, indent=2)
            file.write("\n")
        status.update(prepared=True, phase="prepared", current=None)
        return status
    except (Exception, KeyboardInterrupt) as error:
        # Never persist transport exceptions: they can contain signed CDN URLs.
        if isinstance(error, PreparationError):
            code = error.code
        elif isinstance(error, KeyboardInterrupt):
            code = "interrupted"
        else:
            code = "transfer"
        status.update(phase="failed", error={"code": code, "path": status["current"]})
        raise PreparationError(code) from None
    finally:
        save()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--reuse-models", type=Path)
    args = parser.parse_args()
    try:
        plan = json.loads(SOURCE_LOCK.read_text(encoding="utf-8"))
        print(json.dumps(prepare(plan, args.output, reuse_models=args.reuse_models)))
    except Exception as error:
        code = error.code if isinstance(error, PreparationError) else "storage"
        print(json.dumps({"prepared": False, "actualExecution": False, "error": {"code": code}}))
        raise SystemExit(1) from None


if __name__ == "__main__":
    main()
