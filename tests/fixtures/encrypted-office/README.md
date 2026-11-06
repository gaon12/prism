# Public Office password fixtures

These files come from
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
