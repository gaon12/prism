"""Opt-in actual managed Linux Office rendering and owned-process cleanup.

Requires the exact-source office QA binary and a private installed pinned ZIP.
This standard-library driver uses real Office; independent PDF/text/pixel
validation is a separate developer check, never a replacement renderer.
"""

import hashlib
import json
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path

from test_managed import two_pages

ROOT = Path(__file__).resolve().parent
REPO = ROOT.parent.parent


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def processes():
    found = {}
    for path in Path("/proc").iterdir():
        if not path.name.isdigit():
            continue
        try:
            text = (path / "stat").read_text()
            tail = text.rsplit(")", 1)[1].split()
            found[int(path.name)] = {
                "pid": int(path.name),
                "name": text.split("(", 1)[1].rsplit(")", 1)[0],
                "parent": int(tail[1]),
                "birth": tail[19],
                "state": tail[0],
            }
        except (FileNotFoundError, ProcessLookupError, PermissionError):
            pass
    return found


def descendants(pid):
    table = processes()
    pending, found = [pid], []
    while pending:
        parent = pending.pop()
        for child in table.values():
            if child["parent"] == parent:
                found.append(child)
                pending.append(child["pid"])
    return found


def live_engine(child):
    deadline = time.monotonic() + 45
    while time.monotonic() < deadline:
        assert child.poll() is None, (
            "The actual request ended before an engine was observed"
        )
        owned = descendants(child.pid)
        if any(p["name"] == "soffice.bin" and p["state"] != "Z" for p in owned):
            return owned
        time.sleep(0.01)
    raise AssertionError("Actual owned soffice.bin never appeared")


def ended(owned):
    table = processes()
    return all(
        p["pid"] not in table
        or table[p["pid"]]["birth"] != p["birth"]
        or table[p["pid"]]["state"] == "Z"
        for p in owned
    )


def no_runtime_children(runtime, data):
    # Scope observations to this exact private executable tree AND work path.
    # Never terminate a process found by name, PID cache or this enumeration.
    for process in processes().values():
        if process["state"] == "Z":
            continue
        entry = Path(f"/proc/{process['pid']}")
        try:
            executable = (entry / "exe").resolve(strict=True)
            if (
                executable.is_relative_to(runtime)
                and str(data / "work").encode() in (entry / "cmdline").read_bytes()
            ):
                return False
        except (FileNotFoundError, ProcessLookupError, PermissionError):
            pass
    return True


def main():
    assert sys.platform == "linux"
    executable = Path(os.environ["PRISM_OFFICE_QA_EXE"]).resolve(strict=True)
    data = Path(os.environ["PRISM_OFFICE_QA_DATA"]).resolve(strict=True)
    assert (data / "qa-owner.txt").read_bytes() == b"Prism Office native QA data v1\n"
    catalog = json.loads((ROOT / "linux-x64.lock.json").read_text(encoding="utf-8"))
    runtime = data / f"libreoffice-linux-x64-{catalog['inventorySha256'][:16]}"
    initial_work = {path.name for path in (data / "work").iterdir()}
    configured = os.environ.get("PRISM_OFFICE_QA_OUTPUT")
    if configured:
        output = Path(configured)
        output.mkdir(parents=True, exist_ok=False)
    else:
        output = Path(tempfile.mkdtemp(prefix="prism-linux-office-result-"))
    worker = hashlib.sha256(
        (ROOT / "worker.py").read_bytes() + (ROOT / "unix_owner.py").read_bytes()
    ).hexdigest()
    expected = catalog["inventorySha256"] + ":" + worker
    sources = {
        entry["file"]: entry
        for entry in json.loads(
            (REPO / "tests/fixtures/encrypted-office/sources.json").read_text()
        )
    }
    synthetic = output / "two-pages.docx"
    two_pages(REPO / "tests/fixtures/encrypted-office/example.docx", synthetic)
    results = []
    for path in [
        *[
            REPO / "tests/fixtures/encrypted-office" / name
            for name in ["example.docx", "reference-slides.pptx", "example.xlsx"]
        ],
        synthetic,
    ]:
        before = sha(path)
        if path.name in sources:
            assert before == sources[path.name]["sha256"].lower()
        target = output / (path.name + ".pdf")
        child = subprocess.run(
            [str(executable), "render", str(data), str(path), str(target)],
            capture_output=True,
            timeout=120,
            check=False,
        )
        assert child.returncode == 0, (path.name, child.stderr.decode(errors="replace"))
        result = json.loads(child.stdout)
        assert result["rendered"] and result["fingerprint"] == expected
        assert (
            target.read_bytes().startswith(b"%PDF-")
            and target.stat().st_size == result["pdfBytes"]
        )
        assert sha(path) == before
        assert {path.name for path in (data / "work").iterdir()} == initial_work
        results.append(
            {
                "source": path.name,
                "sourceSha256": before,
                "pdf": str(target),
                "pdfSha256": sha(target),
                "metadata": result,
            }
        )
    lifetime = []
    canary = subprocess.Popen(
        [sys.executable, "-c", "import time; time.sleep(120)"], start_new_session=True
    )
    try:
        for mode in ["cancel", "hard-parent-death"]:
            target = output / (mode + ".pdf")
            child = subprocess.Popen(
                [
                    str(executable),
                    "render-cancel" if mode == "cancel" else "render",
                    str(data),
                    str(REPO / "tests/fixtures/encrypted-office/example.docx"),
                    str(target),
                ],
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
            try:
                owned = live_engine(child)
                started = time.monotonic()
                if mode == "cancel":
                    (data / f"qa-cancel-{child.pid}.flag").write_bytes(
                        b"cancel owned QA request\n"
                    )
                else:
                    child.kill()  # This exact held, unreaped Child only.
                stdout, _stderr = child.communicate(timeout=15)
                if mode == "cancel":
                    assert child.returncode == 0 and json.loads(stdout)["cancelled"]
                    (data / f"qa-cancel-{child.pid}.flag").unlink()
                else:
                    assert child.returncode == -9
                deadline = time.monotonic() + 8
                while time.monotonic() < deadline and (
                    not ended(owned) or not no_runtime_children(runtime, data)
                ):
                    time.sleep(0.02)
                assert ended(owned) and no_runtime_children(runtime, data)
                assert canary.poll() is None
                assert not target.exists(), "A cancelled request must not publish a PDF"
                if mode == "cancel":
                    assert {
                        path.name for path in (data / "work").iterdir()
                    } == initial_work
                lifetime.append(
                    {
                        "mode": mode,
                        "elapsedMs": round((time.monotonic() - started) * 1000),
                        "owned": owned,
                        "ownedEnded": True,
                        "canaryAlive": True,
                    }
                )
            finally:
                if child.poll() is None:
                    child.kill()
                    child.wait(timeout=8)
                child.stdout.close()
                child.stderr.close()
    finally:
        canary.terminate()
        canary.wait(timeout=5)
    record = {
        "passed": True,
        "output": str(output),
        "data": str(data),
        "executableSha256": sha(executable),
        "fingerprint": expected,
        "results": results,
        "lifetime": lifetime,
        "hardDeathRetainedWork": [path.name for path in (data / "work").iterdir()],
        "initialRetainedWork": sorted(initial_work),
        "scope": "Linux CPU/runtime under WSL; independent PDF validation is separate, no desktop/AppImage proof.",
    }
    (output / "result.json").write_text(json.dumps(record, indent=2), encoding="utf-8")
    print(json.dumps(record))


if __name__ == "__main__":
    main()
