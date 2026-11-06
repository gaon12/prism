# HWPX export engine checks

This independent crate compiles the exact production `document_hwpx.rs` and
Markdown preparation modules without Tauri, LanceDB, Hancom, Python or an asset
resolver. It does not enable an export button in the application.

```sh
cargo test --manifest-path tools/hwpx-export-qa/Cargo.toml --locked
cargo clippy --manifest-path tools/hwpx-export-qa/Cargo.toml --locked --all-targets -- -D warnings
cargo run --manifest-path tools/hwpx-export-qa/Cargo.toml --locked -- /absolute/new-report.hwpx
```

The command renders a public synthetic fixture and uses create-new semantics;
it never replaces an existing destination. Tests produce actual HWPX packages
with multilingual text, tables, code and footnotes. They check visible
classification, code without Markdown backticks, actual section-to-header
references for monospace formatting and column alignment, empty cells, unused notes, unsafe resource
references, cancellation, deadlines, depth/input limits and repeated-note
expansion bounds. The generated package is also decoded by the codec. A decoder
round trip alone does not prove compatibility with a native word processor.

The adapter uses HwpForge 0.19.1 (source revision
`27547f4499a515be4b039079d4a5355dea8d39f7`) with exact codec/core/foundation
versions and a committed lockfile. The Markdown decoder is used with a fixed
built-in template, frontmatter configuration is disabled, and separate image
loading helpers are never called. Images become visible descriptions; raw HTML
is literal text; unsafe links lose their target. Unexpected codec warnings fail
the export instead of silently publishing lost content. Short-lived opaque
markers preserve code spans and table alignment through the typed model; all
markers must be consumed and absent from the produced package.

The crate also compiles the shared destination writer in its test target. Real
temporary-directory checks cover interrupted writes and commit cancellation,
hardlink preservation, staging-name collisions, invalid output paths and bounds.
Unix additionally checks symlink rejection and owner-only staging permissions.

The caller must retain its actual export admission while a codec call is still
running after cancellation or timeout, and publish only through a cancellable
atomic destination writer. This module returns bytes; it never reads source
files or writes the selected destination. The limits are resource controls, not
a process security sandbox.

Native Hancom rendering, font substitution, full schema conformance, complete
Markdown fidelity and integration with saved application revisions remain
separate verification gates. These tests must not be used to claim them. The
current adapter declares a Latin monospace face; it does not embed a font file
in HWPX. Preserve this distinction from the PDF engine's bundled fonts.

References: [Hancom package structure](https://tech.hancom.com/hwpxformat/),
[official OWPML model](https://github.com/hancom-io/hwpx-owpml-model), and
[HwpForge source](https://github.com/ai-screams/HwpForge/tree/27547f4499a515be4b039079d4a5355dea8d39f7).
HwpForge is used under its [MIT license](LICENSE-HwpForge-MIT.txt).

본 제품은 한컴의 HWP 문서 파일(.hwp) 공개 문서를 참고하여 개발하였습니다.
When exposing this engine in the app, carry the Hancom attribution and library
notice into the relevant UI/help and packaged notices as well as the source.
