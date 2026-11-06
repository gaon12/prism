"""Actual external-image canary, not a network sandbox or all-format proof."""

import base64
import hashlib
import io
import json
import os
import tempfile
import threading
import urllib.request
import xml.etree.ElementTree as ET
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from zipfile import ZIP_DEFLATED, ZipFile

from test_actual import REPO, finish, start


def package(source, url):
    with ZipFile(io.BytesIO(source)) as archive:
        parts = {entry.filename: archive.read(entry) for entry in archive.infolist()}
    relation = (
        '<Relationship Id="rId999" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" '
        f'TargetMode="External" Target="{url}"/>'
    ).encode()
    relationships = parts["word/_rels/document.xml.rels"]
    assert relationships.count(b"</Relationships>") == 1
    parts["word/_rels/document.xml.rels"] = relationships.replace(
        b"</Relationships>", relation + b"</Relationships>"
    )
    # Preserve original namespace declarations, including unused prefixes named
    # by mc:Ignorable. Re-serializing the entire XML would drop those bindings.
    paragraph = b"""<w:p xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"
      xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"
      xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"
      xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
      xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture">
      <w:r><w:drawing><wp:inline><wp:extent cx="914400" cy="914400"/><wp:docPr id="999" name="Local fixture"/>
      <a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic>
      <pic:nvPicPr><pic:cNvPr id="0" name="local.png"/><pic:cNvPicPr/></pic:nvPicPr>
      <pic:blipFill><a:blip r:link="rId999"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill>
      <pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="914400" cy="914400"/></a:xfrm>
      <a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic>
      </wp:inline></w:drawing></w:r></w:p>"""
    document = parts["word/document.xml"]
    assert document.count(b"<w:body>") == 1
    parts["word/document.xml"] = document.replace(b"<w:body>", b"<w:body>" + paragraph)
    ET.fromstring(parts["word/document.xml"])
    ET.fromstring(parts["word/_rels/document.xml.rels"])
    result = io.BytesIO()
    with ZipFile(result, "w", ZIP_DEFLATED) as archive:
        for name, data in parts.items():
            archive.writestr(name, data)
    return result.getvalue()


def main():
    from pypdf import PdfReader

    runtime = Path(os.environ["PRISM_OFFICE_QA_RUNTIME"]).resolve(strict=True)
    root = Path(tempfile.mkdtemp(prefix="prism-office-external-"))
    requests = []
    image = base64.b64decode(
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9Wl6ks8AAAAASUVORK5CYII="
    )

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            requests.append(self.path)
            self.send_response(200)
            self.send_header("Content-Type", "image/png")
            self.end_headers()
            self.wfile.write(image)

        def log_message(self, *_args):
            pass

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        # Establish that the canary is reachable before checking the engine.
        base = f"http://127.0.0.1:{server.server_port}"
        with urllib.request.urlopen(base + "/health", timeout=5) as response:
            assert response.read() == image
        assert requests == ["/health"]
        requests.clear()
        source = REPO / "tests/fixtures/encrypted-office/example.docx"
        original = source.read_bytes()
        document = package(original, base + "/prism-image-canary.png")
        (root / "external-picture.docx").write_bytes(document)
        metadata, pdf = finish(start(runtime, root / "worker", document, "docx"))
        assert "Lorem ipsum" in PdfReader(io.BytesIO(pdf)).pages[0].extract_text()
        (root / "result.pdf").write_bytes(pdf)
        assert requests == [], "External source image was requested during conversion"
        assert source.read_bytes() == original
        print(
            json.dumps(
                {
                    "passed": True,
                    "root": str(root),
                    "externalImageRequests": 0,
                    "canaryReachable": True,
                    "sourceSha256": hashlib.sha256(document).hexdigest(),
                    "metadata": metadata,
                }
            )
        )
    finally:
        server.shutdown()
        server.server_close()


if __name__ == "__main__":
    main()
