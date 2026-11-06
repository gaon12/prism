# Independent EPUB fixtures

`generate.py` uses Python's standard-library ZIP, PNG and compression primitives.
It does not call Prism, its parser, a browser renderer or an EPUB helper library.
The documents contain public generated text and color pixels, without user data.

- `spine.epub`: ZIP and manifest order put the second chapter first. The declared
  cover is green; the actual first spine document has red pixels and bilingual
  text. Local CSS includes an import and percent-encoded relative image paths.
  Its second document contains a table, ruby and 100 original paragraphs. Script,
  iframe and image-handler traps target only an explicit loopback test address.
- `svg.epub`: the same book with a primary SVG first document. Its 200×300
  `viewBox` determines the viewport despite percentage width/height attributes.

The [EPUB specification](https://www.w3.org/TR/epub-33/) defines the container,
package and spine. These small fixtures exercise those mechanisms; they are not
a full reading-system conformance suite. Recreate them with:

```sh
python tests/fixtures/epub/generate.py
```

`generate-navigation.py` independently creates `navigation.epub` (EPUB 3
hierarchical navigation with an alternate namespace prefix and mixed inline
labels), `navigation-ncx.epub` (legacy NCX), `navigation-spine.epub` (no
declared contents), and `navigation-many.epub` (240 searchable entries).
ZIP/manifest order differs from the spine. They include a
percent-encoded Korean fragment far down a long section, a missing fragment,
and external, escaping and non-spine targets. No user text is included.

The platform QA unit tests check order, XML/path/size/cancellation errors, CSS
token rewriting/import cycles, encryption diagnostics and authenticated cache
admission. Deterministic 1,600-byte font payloads test both IDPF and Adobe
metadata/key/boundary handling; they do not substitute for real font rendering.

`npm run test:collections-epub` opens an isolated bundled Windows WebView. It
imports the actual files through native IPC, checks actual WebP pixels and
text/table ink, author styles, sandboxing, first-page identity, keyboard reading,
responsive layouts, bookmark/cache persistence, current-source rejection,
reference removal and unchanged scope permissions/originals. The fixture's
loopback address must receive zero requests. Exact native document-start
sandbox-denial logs are recorded separately from application errors.

The native test needs the fixture's loopback port 38491 free. It never stops an
unrelated process to obtain that port. The OS picker UI, full EPUB conformance,
actual embedded-font output and Mac/Linux desktop rendering remain unverified.
