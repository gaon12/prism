"""Opt-in Windows test of real engine pixels and owned-process cancellation.

Requires PRISM_OFFICE_QA_RUNTIME and developer pypdf/pypdfium2/Pillow packages.
It reads only the repository's public synthetic Office fixtures.
"""

import ctypes
import hashlib
import io
import json
import os
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent
REPO = ROOT.parent.parent
MAX_OUTPUT = (64 << 20) + 8192
OWNER = b"Prism Office preview worker v1\n"


def start(runtime, directory, source, extension):
    directory.mkdir()
    (directory / "owner.txt").write_bytes(OWNER)
    interpreter = runtime / "program/python-core-3.13.17/bin/python.exe"
    environment = dict(
        os.environ, PATH=str(runtime / "program") + os.pathsep + os.environ["PATH"]
    )
    child = subprocess.Popen(
        [
            str(interpreter),
            "-I",
            "-S",
            "-B",
            str(ROOT / "worker.py"),
            str(runtime),
            str(directory),
        ],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=environment,
        creationflags=0x08000000,
    )
    packet = json.dumps(
        {"version": 1, "extension": extension, "bytes": len(source)}
    ).encode()
    child.stdin.write(packet + b"\n" + source)
    child.stdin.flush()
    return child


def finish(child):
    output, errors = [], []
    threads = []
    for pipe, target, limit in [
        (child.stdout, output, MAX_OUTPUT),
        (child.stderr, errors, 4096),
    ]:

        def read(pipe=pipe, target=target, limit=limit):
            data = pipe.read(limit + 1)
            if len(data) > limit:
                child.kill()
            target.append(data)

        thread = threading.Thread(target=read, daemon=True)
        thread.start()
        threads.append(thread)
    try:
        code = child.wait(timeout=90)
    except subprocess.TimeoutExpired:
        child.kill()
        child.wait()
        raise
    finally:
        child.stdin.close()
    for thread in threads:
        thread.join(timeout=5)
        assert not thread.is_alive()
    assert code == 0, (code, output, errors)
    assert errors == [b""]
    header, pdf = output[0].split(b"\n", 1)
    metadata = json.loads(header)
    assert metadata["version"] == 1 and metadata["ok"]
    assert metadata["pdfBytes"] == len(pdf) and pdf.startswith(b"%PDF-")
    return metadata, pdf


def descendants(root):
    # Read only process ancestry; never signal a PID obtained by enumeration.
    from ctypes import wintypes as w

    class Entry(ctypes.Structure):
        _fields_ = [
            ("size", w.DWORD),
            ("usage", w.DWORD),
            ("pid", w.DWORD),
            ("heap", ctypes.c_size_t),
            ("module", w.DWORD),
            ("threads", w.DWORD),
            ("parent", w.DWORD),
            ("priority", w.LONG),
            ("flags", w.DWORD),
            ("name", w.WCHAR * 260),
        ]

    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.CreateToolhelp32Snapshot.argtypes = [w.DWORD, w.DWORD]
    kernel.CreateToolhelp32Snapshot.restype = w.HANDLE
    kernel.Process32FirstW.argtypes = [w.HANDLE, ctypes.POINTER(Entry)]
    kernel.Process32NextW.argtypes = [w.HANDLE, ctypes.POINTER(Entry)]
    kernel.CloseHandle.argtypes = [w.HANDLE]
    handle = kernel.CreateToolhelp32Snapshot(2, 0)
    assert handle and handle != ctypes.c_void_p(-1).value
    mapping = {}
    try:
        entry = Entry()
        entry.size = ctypes.sizeof(entry)
        available = kernel.Process32FirstW(handle, ctypes.byref(entry))
        while available:
            mapping[entry.pid] = (entry.parent, entry.name)
            available = kernel.Process32NextW(handle, ctypes.byref(entry))
    finally:
        kernel.CloseHandle(handle)
    selected = {root}
    while True:
        found = selected | {
            pid for pid, (parent, _) in mapping.items() if parent in selected
        }
        if found == selected:
            return {pid: mapping[pid][1] for pid in selected if pid in mapping}
        selected = found


def open_live_handles(parent, excluded=None):
    from ctypes import wintypes as w

    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.OpenProcess.argtypes = [w.DWORD, w.BOOL, w.DWORD]
    kernel.OpenProcess.restype = w.HANDLE
    kernel.GetProcessTimes.argtypes = [
        w.HANDLE,
        ctypes.POINTER(w.FILETIME),
        ctypes.POINTER(w.FILETIME),
        ctypes.POINTER(w.FILETIME),
        ctypes.POINTER(w.FILETIME),
    ]
    kernel.WaitForSingleObject.argtypes = [w.HANDLE, w.DWORD]
    kernel.CloseHandle.argtypes = [w.HANDLE]

    def created(handle):
        times = [w.FILETIME() for _ in range(4)]
        assert kernel.GetProcessTimes(handle, *[ctypes.byref(t) for t in times])
        return times[0].dwLowDateTime | times[0].dwHighDateTime << 32

    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        found = descendants(parent)
        if "soffice.bin" in found.values():
            # PPIDs outlive parents and may point to a newly reused PID. An old
            # console with that stale PPID is not our descendant. Retain handles
            # and compare creation times before treating anything as owned.
            handles, names = [], []
            try:
                root = kernel.OpenProcess(0x101000, False, parent)
                assert root, "Open the live owned root before cancellation"
                handles.append(root)
                names.append(found[parent])
                birth = created(root)
                for pid, name in found.items():
                    if pid == parent:
                        continue
                    handle = kernel.OpenProcess(0x101000, False, pid)
                    assert handle, "Open concrete live handles before cancellation"
                    try:
                        old = created(handle) < birth
                    except BaseException:
                        kernel.CloseHandle(handle)
                        raise
                    if old:
                        if excluded is not None:
                            excluded.append(
                                {"name": name, "reason": "createdBeforeRoot"}
                            )
                        kernel.CloseHandle(handle)
                    else:
                        handles.append(handle)
                        names.append(name)
                assert "soffice.bin" in names, "The actual owned engine must be alive"
                return kernel, handles, names
            except BaseException:
                for handle in handles:
                    kernel.CloseHandle(handle)
                raise
        time.sleep(0.05)
    raise AssertionError("Actual engine did not start")


def await_handles(kernel, handles, names=None):
    try:
        for index, handle in enumerate(handles):
            result = kernel.WaitForSingleObject(handle, 5000)
            assert result == 0, (
                "Owned process survived cancellation",
                names[index] if names is not None else index,
                result,
            )
    finally:
        for handle in handles:
            kernel.CloseHandle(handle)


def main():
    assert sys.platform == "win32"
    runtime = Path(os.environ["PRISM_OFFICE_QA_RUNTIME"]).resolve(strict=True)
    root = Path(tempfile.mkdtemp(prefix="prism-office-actual-"))
    if len(sys.argv) == 3 and sys.argv[1] == "--parent-helper":
        # The outer test kills this exact owned parent, not its worker. Closing
        # its last stdin writer must then make the worker end the entire engine.
        source = (REPO / "tests/fixtures/encrypted-office/example.docx").read_bytes()
        child = start(runtime, root / "hard-parent", source, "docx")
        print(json.dumps({"pid": child.pid}), flush=True)
        child.wait(timeout=90)
        return

    import pypdfium2
    from pypdf import PdfReader

    cases = [
        ("docx", "example.docx", "Lorem ipsum"),
        ("pptx", "reference-slides.pptx", "Cell text 1"),
        ("xlsx", "example.xlsx", "lorem ipsum"),
    ]
    results = []
    sources = {
        entry["file"]: entry
        for entry in json.loads(
            (REPO / "tests/fixtures/encrypted-office/sources.json").read_text(
                encoding="utf-8"
            )
        )
    }
    for extension, name, expected in cases:
        source = REPO / "tests/fixtures/encrypted-office" / name
        data = source.read_bytes()
        digest = hashlib.sha256(data).hexdigest()
        assert digest == sources[name]["sha256"].lower()
        assert len(data) == sources[name]["bytes"]
        directory = root / extension
        child = start(runtime, directory, data, extension)
        metadata, pdf = finish(child)
        document = PdfReader(io.BytesIO(pdf))
        assert expected in "\n".join(page.extract_text() for page in document.pages)
        (directory / "result.pdf").write_bytes(pdf)
        with pypdfium2.PdfDocument(pdf) as rendered:
            page = rendered[0]
            bitmap = page.render(scale=1.5)
            image = bitmap.to_pil().convert("RGB")
            image.thumbnail((480, 480))
            assert sum(image.convert("L").histogram()[:245]) > 100
            image.save(directory / "first-page.webp", format="WEBP", lossless=True)
            bitmap.close()
            page.close()
        assert hashlib.sha256(source.read_bytes()).hexdigest() == digest
        results.append(
            {
                "extension": extension,
                "sourceSha256": digest,
                "pages": len(document.pages),
                "pdfSha256": hashlib.sha256(pdf).hexdigest(),
                "metadata": metadata,
                "firstPageWebp": str(directory / "first-page.webp"),
            }
        )

    lifetime = []
    canary = subprocess.Popen(
        [sys.executable, "-c", "import time; time.sleep(90)"], creationflags=0x08000000
    )
    helper = None
    try:
        source = (REPO / "tests/fixtures/encrypted-office/example.docx").read_bytes()
        for mode in ["eof", "control"]:
            child = start(runtime, root / mode, source, "docx")
            kernel, handles, names = open_live_handles(child.pid)
            if mode == "eof":
                child.stdin.close()
            else:
                child.stdin.write(b"!")
                child.stdin.flush()
            assert child.wait(timeout=10) == 72
            await_handles(kernel, handles, names)
            child.stdin.close()
            child.stdout.close()
            child.stderr.close()
            assert canary.poll() is None
            lifetime.append(
                {"mode": mode, "actualProcessesEnded": names, "canaryAlive": True}
            )
        helper = subprocess.Popen(
            [sys.executable, "-B", __file__, "--parent-helper", "1"],
            env=os.environ,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            creationflags=0x08000000,
        )
        worker = json.loads(helper.stdout.readline())["pid"]
        kernel, handles, names = open_live_handles(worker)
        helper.kill()
        helper.wait(timeout=5)
        await_handles(kernel, handles, names)
        assert canary.poll() is None
        lifetime.append(
            {
                "mode": "hard-parent-death",
                "actualProcessesEnded": names,
                "canaryAlive": True,
            }
        )
    finally:
        if helper is not None and helper.poll() is None:
            helper.kill()
            helper.wait()
        if canary.poll() is None:
            canary.kill()
            canary.wait()
    print(
        json.dumps(
            {
                "passed": True,
                "root": str(root),
                "results": results,
                "lifetime": lifetime,
            }
        )
    )


if __name__ == "__main__":
    main()
