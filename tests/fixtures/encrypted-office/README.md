# Public Office password fixtures

The DOCX/XLSX files come from
[`nolze/msoffcrypto-tool` at bdb71ea5dfbdee2f3cd86f8852e74be7a6f15667](https://github.com/nolze/msoffcrypto-tool/tree/bdb71ea5dfbdee2f3cd86f8852e74be7a6f15667/tests).
They contain public synthetic example documents. Their public test password is
`Password1234_`; no user password or private document is stored here.

| Encrypted input                   | Complete expected plaintext           |
| --------------------------------- | ------------------------------------- |
| `example_password.docx`           | `example.docx`                        |
| `example_password.xlsx`           | `example.xlsx`                        |
| `ecma376agile_sha1_password.xlsx` | `example.xlsx`                        |
| `ecma376standard_password.docx`   | `ecma376standard_password_plain.docx` |

[`sources.json`](sources.json) records the pinned source URLs, Git blob IDs,
SHA-256 hashes and byte lengths. Each file was checked against both its inspected
Git blob and the exact pinned upstream source archive. Preserve the original
bytes; tests compare the actual engine output with the **entire** independent
plaintext, not only a ZIP prefix or a few matching words. The upstream project is
distributed under the included [MIT license](LICENSE-msoffcrypto-tool.txt).

The independent Python reader at that same revision also decrypts all four
inputs to exactly these bytes with password and, for Agile, integrity checks.
Its 6.1.0 wheel predates the SHA-1 verifier fix: use the pinned revision when
repeating this additional reference check. That dependency belongs to developer
QA, not the application. These fixtures do not establish native Word/Excel
layout fidelity or support for every Office encryption variant.

The additional `reference-slides.pptx` is the unmodified public
[`python-pptx` slide fixture at 278b47b1dedd5b46ee84c286e77cdfb0bf4594be](https://github.com/scanny/python-pptx/blob/278b47b1dedd5b46ee84c286e77cdfb0bf4594be/tests/test_files/test_slides.pptx),
under the included [MIT license](LICENSE-python-pptx.txt). Its Git blob and
SHA-256 are also pinned in `sources.json`. The separate Python implementation
above encrypted these bytes into `agile-unicode-slides.pptx` with the public test
password `암호-fixture-密碼🔐`, then independently decrypted it with password and
integrity verification and compared the entire result with the original. The
Rust presentation test makes that same whole-package comparison. No Microsoft
Office installation or slide rendering is part of this cryptographic check.
