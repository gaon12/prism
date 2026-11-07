"""Opt-in crash recovery through the exact Rust CLI and real managed Office.

Only the newly observed v2 workspace's marker time is aged; old fixtures and
user originals are never modified. The clock change simulates retention expiry,
not passage of 24 real hours. No renderer or process-lifetime result is mocked.
"""

import hashlib
import json
import os
import subprocess
import sys
import threading
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent
REPO = ROOT.parent.parent
OWNER = b"Prism Office preview worker v2\n"


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    assert sys.platform in {"win32", "linux"}
    executable = Path(os.environ["PRISM_OFFICE_QA_EXE"]).resolve(strict=True)
    data = Path(os.environ["PRISM_OFFICE_QA_DATA"]).resolve(strict=True)
    assert (data / "qa-owner.txt").read_bytes() == b"Prism Office native QA data v1\n"
    work = data / "work"
    initial = {entry.name for entry in work.iterdir()}
    initial_markers = {
        name: (work / name / "owner.txt").read_bytes() for name in initial
    }
    output = Path(os.environ["PRISM_OFFICE_QA_OUTPUT"])
    output.mkdir(parents=True, exist_ok=False)
    source = REPO / "tests/fixtures/encrypted-office/example.docx"
    source_hash = sha(source)
    sources = json.loads((source.parent / "sources.json").read_text(encoding="utf-8"))
    assert source_hash == next(
        item["sha256"].lower() for item in sources if item["file"] == source.name
    )
    catalog = json.loads(
        (
            ROOT
            / (
                "windows-x64.lock.json"
                if sys.platform == "win32"
                else "linux-x64.lock.json"
            )
        ).read_text(encoding="utf-8")
    )
    worker = (ROOT / "worker.py").read_bytes()
    if sys.platform == "linux":
        worker += (ROOT / "unix_owner.py").read_bytes()
    expected = catalog["inventorySha256"] + ":" + hashlib.sha256(worker).hexdigest()
    flags = {"creationflags": 0x08000000} if sys.platform == "win32" else {}
    canary = subprocess.Popen(
        [sys.executable, "-B", "-c", "import time; time.sleep(300)"], **flags
    )
    child = subprocess.Popen(
        [
            str(executable),
            "render",
            str(data),
            str(source),
            str(output / "must-not-exist.pdf"),
        ],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        **flags,
    )
    starting = threading.Event()
    diagnostics = []

    def read_diagnostics():
        for line in child.stderr:
            diagnostics.append(line)
            if b"phase=starting" in line:
                starting.set()

    reader = threading.Thread(target=read_diagnostics)
    reader.start()
    try:
        # Inventory verification precedes spawn. The process observer's engine
        # startup timeout must start at the actual starting phase, not while a
        # debug SHA implementation is still hashing a 1.6 GB private runtime.
        until = time.monotonic() + 125
        while not starting.wait(0.05):
            assert child.poll() is None, (child.returncode, b"".join(diagnostics))
            assert time.monotonic() < until, "Request did not reach engine startup"
        if sys.platform == "win32":
            from test_actual import await_handles, open_live_handles

            kernel, handles, owned = open_live_handles(child.pid)
        else:
            from test_linux import ended, live_engine, no_runtime_children

            owned = live_engine(child)
            runtime = data / f"libreoffice-linux-x64-{catalog['inventorySha256'][:16]}"
        # Only this exact held, unreaped Child receives termination. Ancestry
        # enumeration is read-only and never authorizes killing another process.
        child.kill()
        child.wait(timeout=15)
        assert child.returncode != 0
        assert not (output / "must-not-exist.pdf").exists()
        if sys.platform == "win32":
            await_handles(kernel, handles, owned)
        else:
            until = time.monotonic() + 8
            while time.monotonic() < until and (
                not ended(owned) or not no_runtime_children(runtime, data)
            ):
                time.sleep(0.02)
            assert ended(owned) and no_runtime_children(runtime, data)
        added = {entry.name for entry in work.iterdir()} - initial
        assert len(added) == 1, added
        name = added.pop()
        assert (
            name.startswith("r-")
            and len(name) == 34
            and all(c in "0123456789abcdef" for c in name[2:])
        )
        request = work / name
        assert request.resolve(strict=True).parent == work.resolve(strict=True)
        assert (
            not request.is_symlink()
            and not getattr(request, "is_junction", lambda: False)()
        )
        marker = request / "owner.txt"
        assert not marker.is_symlink() and marker.read_bytes() == OWNER
        results = []
        for phase in ["recent-retained", "aged-recovered"]:
            if phase == "aged-recovered":
                old = time.time() - 24 * 60 * 60 - 60
                os.utime(marker, (old, old))
            pdf = output / f"{phase}.pdf"
            render = subprocess.run(
                [str(executable), "render", str(data), str(source), str(pdf)],
                capture_output=True,
                timeout=150,
                check=True,
                **flags,
            )
            metadata = json.loads(render.stdout)
            assert metadata["rendered"] and metadata["fingerprint"] == expected
            assert (
                pdf.read_bytes().startswith(b"%PDF-")
                and pdf.stat().st_size == metadata["pdfBytes"]
            )
            assert canary.poll() is None
            assert {entry.name for entry in work.iterdir()} == (
                initial | {name} if phase == "recent-retained" else initial
            )
            for previous, contents in initial_markers.items():
                assert (work / previous / "owner.txt").read_bytes() == contents
            results.append(
                {"phase": phase, "metadata": metadata, "pdfSha256": sha(pdf)}
            )
        assert sha(source) == source_hash
        record = {
            "passed": True,
            "platform": sys.platform,
            "executableSha256": sha(executable),
            "sourceSha256": source_hash,
            "fingerprint": expected,
            "hardParentDeath": True,
            "ownedEnded": owned,
            "canaryAlive": True,
            "initialWorkPreserved": sorted(initial),
            "agedMarkerOnly": name,
            "results": results,
        }
        (output / "result.json").write_text(
            json.dumps(record, indent=2), encoding="utf-8"
        )
        print(json.dumps(record))
    finally:
        if child.poll() is None:
            child.kill()
            child.wait(timeout=15)
        reader.join(timeout=5)
        assert not reader.is_alive()
        (output / "parent-stderr.log").write_bytes(b"".join(diagnostics))
        child.stdout.close()
        child.stderr.close()
        canary.terminate()
        canary.wait(timeout=5)


if __name__ == "__main__":
    main()
