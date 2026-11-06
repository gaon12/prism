"""Opt-in Windows regression: a real startup failure must not dirty the engine.

This deliberately exceeds the engine's Win32 extension-registry path budget.
Only public synthetic input is used; no crash-memory contents are inspected.
"""

import json
import os
import sys
import tempfile
import threading
from pathlib import Path

from test_actual import MAX_OUTPUT, REPO, start


def main():
    assert sys.platform == "win32"
    runtime = Path(os.environ["PRISM_OFFICE_QA_RUNTIME"]).resolve(strict=True)
    root = Path(tempfile.mkdtemp(prefix="prism-office-crash-guard-"))
    work = root / ("long-" + "a" * 100)
    before = {
        p.relative_to(runtime).as_posix() for p in runtime.rglob("*") if p.is_file()
    }
    # An inherited value would override CrashDumpEnable=false unless removed.
    os.environ["CRASH_DUMP_ENABLE"] = "1"
    child = start(
        runtime,
        work,
        (REPO / "tests/fixtures/encrypted-office/example.docx").read_bytes(),
        "docx",
    )
    output, errors = [], []
    threads = []
    for pipe, target in [(child.stdout, output), (child.stderr, errors)]:
        thread = threading.Thread(
            target=lambda p=pipe, t=target: t.append(p.read(MAX_OUTPUT + 1))
        )
        thread.start()
        threads.append(thread)
    try:
        code = child.wait(timeout=45)
    finally:
        if child.poll() is None:
            child.kill()
            child.wait()
        child.stdin.close()
        for thread in threads:
            thread.join(timeout=5)
            assert not thread.is_alive()
    assert code == 2, code
    metadata = json.loads(output[0])
    assert not metadata["ok"] and metadata["code"] == "unavailable", metadata
    assert errors == [b""]
    after = {
        p.relative_to(runtime).as_posix() for p in runtime.rglob("*") if p.is_file()
    }
    assert before == after, "A failed engine wrote into its sealed installation"
    assert not list(root.rglob("*.dmp")), "Do not persist document-memory crash dumps"
    record = {
        "passed": True,
        "root": str(root),
        "metadata": metadata,
        "engineInventoryUnchanged": True,
        "inheritedDumpEnableRemoved": True,
    }
    (root / "result.json").write_text(json.dumps(record, indent=2), encoding="utf-8")
    print(json.dumps(record))


if __name__ == "__main__":
    main()
