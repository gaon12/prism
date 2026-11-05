# lopdf 0.42.0

Source: https://crates.io/crates/lopdf/0.42.0
Repository: https://github.com/J-F-Liu/lopdf
License: MIT (included in LICENSE).
Published crate SHA-256: 25aab26d99567469098e64a02f42679f8965c6401263eefa31d8f2dcc37a221c

Prism retains the published library source, manifest and attribution. The local
change in src/encryption/algorithms.rs reuses Algorithm 7's existing recovery
and authentication of the padded user password for legacy owner passwords.
Algorithm 2 must derive the file key from that recovered user password, rather
than from the accepted owner password itself. No cipher implementation changes.

Independent pypdf-generated RC4-128 and AES-256 fixtures exercise both password
roles, wrong passwords, page contents, and the unchanged encrypted original in
Prism's document_extract tests. The same patched source is used by application
and standalone platform QA. Re-evaluate this patch when upgrading lopdf.
