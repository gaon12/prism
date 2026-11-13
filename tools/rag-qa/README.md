# RAG and document core checks

This crate imports the current production modules directly from src-tauri/src.
It runs conversation/document persistence, source and provider permissions,
selection snapshots, cancellation, MCP protocol, credential, transport and
document-schema regressions without linking Tauri, Lance, the filename index
or export renderers.

```sh
npm run test:rag-core
cargo clippy --manifest-path tools/rag-qa/Cargo.toml --all-targets --locked -- -D warnings
cargo fmt --manifest-path tools/rag-qa/Cargo.toml --check
```

The npm command compiles with one job and runs two test threads. The tool's dev
and test profiles omit debug symbols while retaining normal assertions and
overflow checks. Use CARGO_NET_OFFLINE=true when all locked dependencies are
already cached. CARGO_TARGET_DIR can choose a different build-output location.
Rust and a C compiler for bundled SQLite are required.

The modules have the same private visibility as in the app. The QA crate allows
dead-code warnings because full-app callers are absent; it does not suppress
other compiler or Clippy diagnostics. No old source copies, substituted search
engines or weakened production checks are used.

Most source resolvers and completion responses in these tests are deliberately
synthetic fixtures. SQLite persistence, native HTTP transport and the default
Windows DPAPI fixture checks execute real implementations. Platform credential
roundtrip tests that require a disposable macOS/Linux OS-store session remain
explicitly ignored unless selected with their documented environment opt-in.
The Windows process-job tests start and terminate only their own windowless
loopback ping children.

The existing ignored native credential test can be selected through
`scripts/qualify-macos-keychain.py` on a **disposable GitHub-hosted macOS runner**
with Python 3.11 or newer:

```sh
python3 -B scripts/qualify-macos-keychain.py --output "$RUNNER_TEMP/prism-keychain-evidence"
```

The script first compiles this crate's actual test binary with locked application
dependency versions. It then creates a private temporary Keychain, selects it as
the User-domain default and sole search entry, and verifies that selection before
running exactly `native_store_roundtrip_replacement_isolation_and_delete`.
The test executes the production native credential provider with random service
names and synthetic keys. Compilation is bounded to 15 minutes and the selected
test to 60 seconds. The original default and search list are restored and checked
even when the test or Keychain creation fails. The temporary Keychain is deleted
only after restoration is verified. No login password or real API key is needed.
Apple's [security tool source](https://github.com/apple-oss-distributions/Security/blob/main/SecurityTool/macOS/security.c)
documents the User-domain default and search-list operations used here.

The output directory must be new and inside `RUNNER_TEMP`. Upload only that
directory's logs and `result.json`; the Keychain itself is outside it. A successful
result requires the exact test to pass once, unchanged source/binary hashes,
restoration and cleanup. The script refuses local and self-hosted machines before
creating output or changing settings. Unit checks use modeled preference calls
and do not prove macOS API behavior. Actual hosted-Mac execution remains pending;
this check does not establish desktop prompting, signing or locked-login UX.

Non-streaming generation can finish inference before sending response headers.
The transport keeps its 180-second total request budget for that wait, then
applies the 45-second idle limit to each response-body chunk. The four-second
connection limit and cancellation polling remain separate. HTTP regressions cover
delayed headers, a stalled body, oversized responses and the total deadline;
actual model behavior still needs the native generation tests.

Document Markdown exports link sources bound to a block even when a model omits
inline citation labels. Missing footnote references appear after the block,
preserving table and code syntax. Existing inline references are not duplicated;
unknown labels cannot acquire a source, and no claim becomes verified.
Code spans/blocks, escaped literal labels, HTML attributes and link/image targets
keep their original Markdown. Tokens inside those regions do not count as visible
source references; a bound source still receives a separate reference outside them.

Document generation distinguishes the user's writing request and reviewed plan
from untrusted source and document text. Body instructions preserve each reviewed
section's language, layout and requested missing-information notes. This is a
prompting rule, not a guarantee of model quality: the actual-generation native
gate checks the requested two-column table and the original factual assertions.
The document editor displays Markdown in textareas. The gate compares those
values with the saved revision and checks actual table cells in an HWPX export
through an independent ZIP/XML reader, rather than expecting an HTML table in
the editor. This developer-only inspection uses Python's standard library.

Document semantic retrieval combines the topic with the user's writing
instruction within the existing 512-byte UTF-8 bound. Keyword retrieval keeps the
topic, so formatting instructions do not become mandatory source words. This also
preserves keyword fallback when embeddings are unavailable. Stored template
bodies and complete revisions remain excluded. Policy fixtures record both query
forms; the actual retriever and model still require full-app qualification.

Saved file-set retrieval uses path-key order for its bounded 256-file batches,
independent of producer arrival order. SQLite regressions cover shuffled sets
across batch boundaries, deselection, root filters and stale revisions while
preserving the stored UI ordinals. The retrieval cursor uses a selected/path-key
index; it does not allocate another complete list of paths.
The file-set selection UI also displays path-key order while its checkbox IDs
retain their original ordinals. Ranked natural-language selections use their
separate stored result order.

The search-engine adapters in rag_search_snapshot.rs remain part of the full
app. This crate does **not** qualify their linkage, filesystem indexing, vector
retrieval, GUI behavior, actual generative-model quality or shipping artifacts.
Keep the full application CI and native tests. The first Windows verification
passed 90 core tests and strict Clippy; other OS executions of this crate have
not yet been qualified.
