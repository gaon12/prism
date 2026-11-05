# Independent OCR fixtures

These are synthetic, public test documents. No user content is included.
`generate.py` uses Pillow, pypdf and cryptography, independently of Prism's Rust
parser and native OCR adapter. Regeneration on Windows uses `malgun.ttf` for the
two literal lines `PRISM LOCAL SEARCH 74829` and `프리즘 문서 검색 74829`.

- `bilingual.png`: opaque black text on white pixels, without text metadata.
- `transparent.png`: the same text over transparent pixels whose hidden RGB is
  black. Recognition must flatten transparency over white.
- `scan.pdf`: one image-only page; no PDF text operators/text layer.
- `mixed.pdf`: typed page 1, scanned page 2, typed page 3, blank page 4. OCR must
  keep original page numbers and insert its result between the two typed pages.
- `scan-locked.pdf`: image-only page encrypted by pypdf with AES-256. The public
  user password is `ocr-fixture-password`; the owner password is
  `ocr-fixture-owner`. Neither password is a real credential.

The generator asserts that scanned/blank pages have no extractable text and that
PNG payloads do not contain the literal strings. OCR tests check actual pixels,
both passwords, raw-source fingerprint/bytes, page order, missing language packs,
resource budgets and absence of a decrypted PDF file.

Run the installed Windows engine integration explicitly (Korean OCR language pack
required; no engine or language pack is downloaded by the test):

```powershell
cargo test --manifest-path tools/platform-qa/Cargo.toml windows_native_ocr_pixels_pdf_passwords_and_resource_errors -- --ignored --nocapture
```

The default test suite does not require a machine-specific OCR installation.
