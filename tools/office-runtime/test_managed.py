"""Opt-in real Windows test through the application's exact Rust transport.

Requires an explicitly installed private QA runtime, a compiled QA executable,
and developer-only pypdf/pypdfium2/Pillow. No product profile is used or changed.
"""

import hashlib
import io
import json
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from unittest.mock import patch
from zipfile import ZipFile

import test_actual
from test_actual import REPO, await_handles, open_live_handles


def sha(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def two_pages(source, destination):
    # A synthetic input, not a substitute renderer. Actual layout and glyphs
    # must come back through the installed engine and independent PDF rasterizer.
    body = """<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
<w:body><w:p><w:r><w:rPr><w:rFonts w:ascii="Noto Sans CJK KR"
w:eastAsia="Noto Sans CJK KR"/><w:sz w:val="40"/></w:rPr>
<w:t>프리즘 컬렉션 첫 번째 페이지</w:t></w:r></w:p>
<w:p><w:r><w:t>Prism first page: original document layout.</w:t></w:r></w:p>
<w:p><w:r><w:br w:type="page"/></w:r></w:p>
<w:p><w:r><w:rPr><w:rFonts w:ascii="Noto Sans CJK KR"
w:eastAsia="Noto Sans CJK KR"/><w:sz w:val="40"/></w:rPr>
<w:t>두 번째 페이지 확인</w:t></w:r></w:p>
<w:p><w:r><w:t>Prism second page: separate page identity.</w:t></w:r></w:p>
<w:sectPr><w:pgSz w:w="11906" w:h="16838"/>
<w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440"/>
</w:sectPr></w:body></w:document>""".encode()
    with ZipFile(source) as original, ZipFile(destination, "x") as output:
        for entry in original.infolist():
            output.writestr(
                entry,
                body if entry.filename == "word/document.xml" else original.read(entry),
            )


def main():
    assert sys.platform == "win32"
    import pypdfium2
    from pypdf import PdfReader

    executable = Path(os.environ["PRISM_OFFICE_QA_EXE"]).resolve(strict=True)
    data = Path(os.environ["PRISM_OFFICE_QA_DATA"]).resolve(strict=True)
    assert (data / "qa-owner.txt").read_bytes() == b"Prism Office native QA data v1\n"
    assert not any((data / "work").iterdir()), "Use an idle, private QA runtime"
    output = Path(tempfile.mkdtemp(prefix="prism-office-managed-"))
    fixtures = REPO / "tests/fixtures/encrypted-office"
    sources = {
        entry["file"]: entry
        for entry in json.loads((fixtures / "sources.json").read_text(encoding="utf-8"))
    }
    synthetic = output / "two-pages.docx"
    two_pages(fixtures / "example.docx", synthetic)
    cases = [
        (fixtures / "example.docx", "Lorem ipsum", 1),
        (fixtures / "reference-slides.pptx", "Cell text 1", 1),
        (fixtures / "example.xlsx", "lorem ipsum", 1),
        (synthetic, "프리즘 컬렉션 첫 번째 페이지", 2),
    ]
    results = []
    for number, (source, expected, page_count) in enumerate(cases):
        before = sha(source)
        if source.parent == fixtures:
            assert before == sources[source.name]["sha256"].lower()
            assert source.stat().st_size == sources[source.name]["bytes"]
        pdf_path = output / f"result-{number}.pdf"
        completed = subprocess.run(
            [str(executable), "render", str(data), str(source), str(pdf_path)],
            capture_output=True,
            timeout=135,
            creationflags=0x08000000,
            check=True,
        )
        result = json.loads(completed.stdout)
        pdf = pdf_path.read_bytes()
        assert result["rendered"] and result["pdfBytes"] == len(pdf)
        document = PdfReader(io.BytesIO(pdf))
        assert len(document.pages) == page_count
        assert expected in document.pages[0].extract_text()
        if page_count == 2:
            assert "두 번째 페이지 확인" in document.pages[1].extract_text()
            assert "두 번째 페이지 확인" not in document.pages[0].extract_text()
        images = []
        with pypdfium2.PdfDocument(pdf) as rendered:
            for index in range(page_count):
                page = rendered[index]
                bitmap = page.render(scale=1.5)
                image = bitmap.to_pil().convert("RGB")
                image.thumbnail((640, 640))
                assert sum(image.convert("L").histogram()[:245]) > 100
                path = output / f"result-{number}-page-{index + 1}.webp"
                image.save(path, format="WEBP", lossless=True)
                images.append({"path": str(path), "sha256": sha(path)})
                bitmap.close()
                page.close()
        assert sha(source) == before
        assert not any((data / "work").iterdir())
        results.append(
            dict(
                result,
                sourceSha256=before,
                pages=page_count,
                pdfSha256=sha(pdf_path),
                images=images,
            )
        )

    canary = subprocess.Popen(
        [sys.executable, "-c", "import time; time.sleep(90)"], creationflags=0x08000000
    )
    child = subprocess.Popen(
        [
            str(executable),
            "render-cancel",
            str(data),
            str(fixtures / "example.docx"),
            str(output / "must-not-exist.pdf"),
        ],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        creationflags=0x08000000,
    )
    trigger = data / f"qa-cancel-{child.pid}.flag"
    try:
        excluded = []
        snapshot = test_actual.descendants

        def stale_parent_fixture(parent):
            # Exercise the reused-PPID boundary with a real older live canary.
            # Only this extra ancestry edge is synthetic; the engine, handles,
            # renderer and cancellation below are all real.
            return {**snapshot(parent), canary.pid: "qa-stale-parent-fixture"}

        with patch.object(test_actual, "descendants", stale_parent_fixture):
            kernel, handles, names = open_live_handles(child.pid, excluded)
        assert {
            "name": "qa-stale-parent-fixture",
            "reason": "createdBeforeRoot",
        } in excluded
        # Preserve useful output even if a later lifetime assertion fails.
        (output / "pending-qualification.json").write_text(
            json.dumps(
                {"results": results, "ownedProcesses": names, "excluded": excluded}
            ),
            encoding="utf-8",
        )
        started = time.monotonic()
        trigger.write_bytes(b"cancel private QA request\n")
        stdout, stderr = child.communicate(timeout=15)
        assert child.returncode == 0, (child.returncode, stdout, stderr)
        assert json.loads(stdout)["cancelled"]
        assert not (output / "must-not-exist.pdf").exists()
        await_handles(kernel, handles, names)
        assert canary.poll() is None
        assert not any((data / "work").iterdir())
        cancelled = {
            "cancelMs": round((time.monotonic() - started) * 1000),
            "ownedProcessesEnded": names,
            "canaryAlive": True,
            "workdirsClean": True,
            "staleAncestryExcluded": excluded,
        }
    finally:
        if child.poll() is None:
            child.kill()
            child.wait()
        if canary.poll() is None:
            canary.kill()
            canary.wait()
        trigger.unlink(missing_ok=True)
    record = {
        "passed": True,
        "root": str(output),
        "executableSha256": sha(executable),
        "workerSha256": sha(REPO / "tools/office-runtime/worker.py"),
        "catalogSha256": sha(REPO / "tools/office-runtime/windows-x64.lock.json"),
        "results": results,
        "cancellation": cancelled,
    }
    (output / "qualification.json").write_text(
        json.dumps(record, indent=2), encoding="utf-8"
    )
    print(json.dumps(record))


if __name__ == "__main__":
    main()
