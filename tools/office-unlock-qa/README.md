# Office password engine checks

This independent crate compiles the exact production `office_unlock.rs` without
Tauri, an Office installation, a provider, a database or Python. The module is not
yet registered in the application and does not enable an Office password button.

```sh
cargo test --manifest-path tools/office-unlock-qa/Cargo.toml --locked
cargo clippy --manifest-path tools/office-unlock-qa/Cargo.toml --locked --all-targets -- -D warnings
```

Tests compare four public encrypted DOCX/XLSX inputs against complete known
plaintext bytes from an independent implementation: Agile SHA-512, Agile SHA-1
and Office 2007 Standard. They also distinguish wrong passwords from changed
ciphertext/missing integrity, preserve a password recipient alongside a
certificate recipient, reject ambiguous password keys, and check input/output,
declared package length, XML depth/entities, KDF cost, cancellation and deadlines.
The [fixture provenance](../../tests/fixtures/encrypted-office/README.md) includes
the independent files, hashes, license and reference-reader revision.

The codec is pinned to `msoffice-crypto = 0.1.0-rc.13`, source revision
`11deb548d81eb255ec7f9c85ffdadb465a526615`, with only `crypto-ops` enabled.
It is a release candidate; the exact dependency graph is locked. Its chosen
[MIT license](LICENSE-msoffice-crypto-MIT.txt) and complete
[upstream notice](NOTICE-msoffice-crypto.txt) are retained here. Carry both into
application notices before shipping the integration.

The adapter reads only borrowed input bytes and writes only an owned in-memory
buffer. It caps the original and declared plaintext sizes before allocating
output, validates all XML KDF declarations before the codec's non-interruptible
hash loop, and checks cancellation/deadlines during stream reads, seeks and
writes. Output storage is allocated once and zeroized on failure/drop. The
caller still owns its password and input; codec internals and process memory
are not a sandbox or a guarantee that no transient copy ever existed.

Agile package integrity is required and verified before returning plaintext.
Standard encryption defines no package HMAC and returns `NotDefined`, never a
fabricated verification claim. Unsupported schemes, corrupt input and rejected
passwords have separate stable codes; third-party diagnostic text is not exposed.

The app caller must retain its real unlock admission while a bounded KDF is
still running after cancellation, keep passwords transient, authenticate the
original encrypted file's fingerprint and current content permissions again
before publication, and avoid writing decrypted files or starting embeddings.
PPTX and additional algorithm cases, full extraction/service/UI wiring, original
file/permission/restart/cancellation tests and HWP/legacy formats remain separate
gates. These core tests alone do not establish them.

Primary format references: [Microsoft MS-OFFCRYPTO key generation](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-offcrypto/74d60145-a0f0-44be-99ce-c65d211b4eb7)
and [segmented payload encryption](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-offcrypto/9e61da63-8ddb-4c0a-b25d-f85d990f44c8).
