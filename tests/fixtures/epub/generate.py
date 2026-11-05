"""Independent EPUB fixture (Python stdlib, no Prism parser or renderer).

ZIP order, manifest order and declared cover deliberately differ from the spine.
The first spine viewport contains actual red pixels and original bilingual text.
"""
from pathlib import Path
import struct
import zlib
import zipfile

root = Path(__file__).resolve().parent


def png(rgb):
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
    pixels = (b"\0" + bytes(rgb) * 60) * 80
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 60, 80, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(pixels)) + chunk(b"IEND", b""))


container = '''<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="OPS/package.opf" media-type="application/oebps-package+xml"/></rootfiles></container>'''
package = '''<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="book-id"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="book-id">urn:uuid:00112233-4455-6677-8899-aabbccddeeff</dc:identifier><dc:title>원문 읽기 순서 &amp; First Page</dc:title><dc:language>ko</dc:language><meta property="rendition:layout">pre-paginated</meta></metadata><manifest><item id="b" href="chapters/10-second.xhtml" media-type="application/xhtml+xml"/><item id="cover" href="images/declared-cover.png" media-type="image/png" properties="cover-image"/><item id="a" href="chapters/2-first.xhtml" media-type="application/xhtml+xml"/><item id="red" href="images/red%20panel.png" media-type="image/png"/><item id="css" href="styles/main.css" media-type="text/css"/><item id="theme" href="styles/theme.css" media-type="text/css"/></manifest><spine page-progression-direction="ltr"><itemref idref="a"/><itemref idref="b" properties="rendition:layout-reflowable"/></spine></package>'''
first = '''<?xml version="1.0"?><html xmlns="http://www.w3.org/1999/xhtml"><head><title>실제 첫 페이지</title><meta name="viewport" content="width=600,height=800"/><link rel="stylesheet" href="../styles/main.css"/></head><body class="opening"><img class="panel" src="../images/red%20panel.png" alt="원본 붉은 그림"/><h1>첫 번째 장 — 原文</h1><p>PRISM EPUB FIRST 74829</p><a href="#end">같은 장 안의 이동</a><script>window.parent.__epubInjected=true;fetch('http://127.0.0.1:38491/script')</script><iframe src="http://127.0.0.1:38491/frame"></iframe><img src="http://127.0.0.1:38491/tracker" onerror="window.parent.__epubInjected=true"/><p id="end">책의 첫 장입니다.</p></body></html>'''
paragraphs = "".join(f"<p>이어지는 원문 단락 {n} — English 日本語 한글.</p>" for n in range(1, 101))
second = f'''<html xmlns="http://www.w3.org/1999/xhtml"><head><title>두 번째 장</title><link rel="stylesheet" href="../styles/main.css"/></head><body><h1>두 번째 장 — Table and Ruby</h1><ruby>漢字<rt>かんじ</rt></ruby><table><tr><th>항목</th><th>수량</th></tr><tr><td>책</td><td>2</td></tr></table>{paragraphs}</body></html>'''
with zipfile.ZipFile(root / "spine.epub", "w") as book:
    book.writestr("mimetype", b"application/epub+zip", compress_type=zipfile.ZIP_STORED)
    for name, data in [
        ("OPS/chapters/10-second.xhtml", second),
        ("META-INF/container.xml", container),
        ("OPS/package.opf", package),
        ("OPS/images/declared-cover.png", png((0, 180, 50))),
        ("OPS/styles/main.css", '@import "theme.css"; body{font-size:20px} .panel{position:absolute;inset:0;width:600px;height:800px;z-index:0} .opening{margin:0} .opening h1,.opening p,.opening a{position:relative;z-index:1;color:white;margin:24px} table{border-collapse:collapse} td,th{border:1px solid #123;padding:8px}'),
        ("OPS/styles/theme.css", 'body{color:#123;background:white;font-family:serif} .opening{background-image:url(../images/red%20panel.png)}'),
        ("OPS/images/red panel.png", png((210, 40, 50))),
        ("OPS/chapters/2-first.xhtml", first),
    ]:
        book.writestr(name, data, compress_type=zipfile.ZIP_DEFLATED)

# A primary SVG spine document uses its own intrinsic viewport, with no HTML
# body margin and no declared-cover substitution.
svg_package = package.replace('href="chapters/2-first.xhtml" media-type="application/xhtml+xml"', 'href="chapters/2-first.svg" media-type="image/svg+xml"')
svg = '''<svg xmlns="http://www.w3.org/2000/svg" width="100%" height="100%" viewBox="0 0 200 300"><image href="../images/red%20panel.png" width="200" height="300"/><text x="12" y="32" fill="white" font-size="18">SVG FIRST 74829</text></svg>'''
with zipfile.ZipFile(root / "spine.epub") as source, zipfile.ZipFile(root / "svg.epub", "w") as book:
    for entry in source.infolist():
        data = source.read(entry.filename)
        if entry.filename == "OPS/package.opf":
            data = svg_package.encode()
        book.writestr(entry, data)
    book.writestr("OPS/chapters/2-first.svg", svg, compress_type=zipfile.ZIP_DEFLATED)
