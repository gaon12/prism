"""Independent, deterministic EPUB navigation fixtures using only Python stdlib."""
from pathlib import Path
from zipfile import ZipFile, ZipInfo, ZIP_STORED, ZIP_DEFLATED

root = Path(__file__).resolve().parent
container = '<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container" version="1.0"><rootfiles><rootfile full-path="OPS/package.opf" media-type="application/oebps-package+xml"/></rootfiles></container>'
first = '<html xmlns="http://www.w3.org/1999/xhtml"><head><title>시작</title></head><body><h1 id="시작">NAV FIRST CHAPTER 32917</h1><p>한국어 소설의 첫 장입니다.</p></body></html>'
paragraphs = ''.join(f'<p>원문 단락 {i} — Korean 日本語 English. 계속 읽는 내용입니다.</p>' for i in range(100))
second = f'<html xmlns="http://www.w3.org/1999/xhtml"><head><title>이어지는 장</title></head><body><h1>두 번째 장</h1>{paragraphs}<h2 id="후반:위치">NAV TARGET ANCHOR 62017</h2>{paragraphs}</body></html>'
nav = '''<html xmlns="http://www.w3.org/1999/xhtml" xmlns:e="http://www.idpf.org/2007/ops"><head><title>목차</title></head><body>
<nav e:type="landmarks"><ol><li><a href="chapters/second.xhtml">목차 아님</a></li></ol></nav>
<nav e:type="toc"><h1>목차</h1><ol><li><span>제1부</span><ol>
<li><a href="chapters/first.xhtml#%EC%8B%9C%EC%9E%91">1 <em>시작</em> 뒤</a></li>
<li><a href="chapters/second.xhtml#%ED%9B%84%EB%B0%98%3A%EC%9C%84%EC%B9%98">후반으로 이동</a></li>
<li><a href="chapters/second.xhtml">장 처음</a></li>
<li><a href="chapters/second.xhtml#missing">없는 위치</a></li>
</ol></li>
<li><a href="https://example.invalid/external">외부 이동 금지</a></li>
<li><a href="../../escape.xhtml">경계 밖 이동 금지</a></li>
<li><a href="notes.xhtml">읽기 순서 밖 문서</a></li>
<li><a href="chapters/first.xhtml#%00">잘못된 위치</a></li>
</ol></nav></body></html>'''
ncx = '''<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1"><navMap>
<navPoint id="a" playOrder="1"><navLabel><text>NCX 시작</text></navLabel><content src="chapters/first.xhtml#%EC%8B%9C%EC%9E%91"/>
<navPoint id="b" playOrder="2"><navLabel><text>NCX 후반</text></navLabel><content src="chapters/second.xhtml#%ED%9B%84%EB%B0%98%3A%EC%9C%84%EC%B9%98"/></navPoint>
</navPoint></navMap></ncx>'''
for name, version, toc in [('navigation', '3.0', 'nav'), ('navigation-ncx', '2.0', 'ncx'), ('navigation-spine', '3.0', ''), ('navigation-many', '3.0', 'many')]:
    extra = '<item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>' if toc in ('nav', 'many') else '<item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>' if toc == 'ncx' else ''
    package = f'''<package xmlns="http://www.idpf.org/2007/opf" version="{version}" unique-identifier="id"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="id">urn:prism:independent-navigation-fixture</dc:identifier><dc:title>목차 검증 소설</dc:title><dc:language>ko</dc:language></metadata><manifest><item id="second" href="chapters/second.xhtml" media-type="application/xhtml+xml"/><item id="first" href="chapters/first.xhtml" media-type="application/xhtml+xml"/><item id="notes" href="notes.xhtml" media-type="application/xhtml+xml"/>{extra}</manifest><spine toc="ncx"><itemref idref="first"/><itemref idref="second"/></spine></package>'''
    entries = [('mimetype', 'application/epub+zip'), ('OPS/chapters/second.xhtml', second), ('META-INF/container.xml', container), ('OPS/package.opf', package), ('OPS/chapters/first.xhtml', first), ('OPS/notes.xhtml', first)]
    if toc == 'nav': entries.append(('OPS/nav.xhtml', nav))
    if toc == 'many':
        links = ''.join(f'<li><a href="chapters/second.xhtml#%ED%9B%84%EB%B0%98%3A%EC%9C%84%EC%B9%98">목차 항목 {i:04d}</a></li>' for i in range(240))
        entries.append(('OPS/nav.xhtml', f'<html xmlns:e="http://www.idpf.org/2007/ops"><body><nav e:type="toc"><ol>{links}</ol></nav></body></html>'))
    if toc == 'ncx': entries.append(('OPS/toc.ncx', ncx))
    with ZipFile(root / (name + '.epub'), 'w') as archive:
        for path, text in entries:
            info = ZipInfo(path, (1980, 1, 1, 0, 0, 0))
            info.compress_type = ZIP_STORED if path == 'mimetype' else ZIP_DEFLATED
            info.external_attr = 0o100644 << 16
            archive.writestr(info, text.encode('utf-8'))
