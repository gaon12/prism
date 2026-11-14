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
Rust and a C compiler for bundled SQLite are required. The complete suite also
requires Node.js on `PATH` for its actual stdio MCP server fixture.

Shared persistence checks import `atomic_file.rs` and `settings_file.rs` directly.
Settings, saved search sessions and update records retain their pretty UTF-8 JSON
format; settings exposes the same `write_json` API. Clipboard files use the same
byte publisher without changing capture, retention, restoration or PNG encoding.
The publisher exclusively creates a unique sibling file, writes and syncs the
complete bytes, then renames it. Ordinary failures clean up only its owned staging
file. Older `.pending` entries are left intact. Callers retain responsibility for
encoding, directory setup and authorization.

At `4776e49`, fixed-pending behavior failed three regressions by rewriting linked
fixture data and leaking staging after a failed publication. The fixed source
passed 138 Windows and 136 Linux core tests (one Linux native-store opt-in ignored),
plus strict all-target Clippy on both. Eleven additional actual Linux filesystem
checks covered the shared publisher and existing Flatpak regressions. Serialization
failure left existing files untouched, and byte/UTF-8 format checks passed.
The actual Windows release app library also passed locked/offline `cargo check`
with its production protocol feature; nine unused/dead-code library warnings were
retained. This is source/type and isolated filesystem verification, not a new
native UI/restart/system-clipboard run, full app CI or power-loss qualification.
Evidence: `.dev/atomic-persistence-20261114/qualification.json`, SHA-256
`92ad54a085012feb13ad36eb632f4f9bc7383b1f2bc5578a280adfe7c791f0c4`.

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

The manual [Mac Keychain qualification workflow](../../.github/workflows/mac-keychain.yml)
runs these checks independently of Release and keeps
`mac-keychain-qualification-<architecture>` logs on success or failure. It has no
release or publishing steps.

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
and do not prove macOS API behavior. The actual production credential API test
passed on both ARM64 and Intel Macs in
[run 43381277889](https://github.com/gaon12/prism/actions/runs/43381277889), with
one selected test per architecture, source/binary integrity, restoration and
cleanup verified. This check does not establish desktop prompting, signing or
locked-login UX.

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
passed 90 core tests and strict Clippy. A later Linux run passed 129 core tests
with one native-store test ignored, plus strict all-target Clippy; the native
store test is selected separately in its disposable session.

## Disposable Linux Secret Service check

With Linux, Python 3.11+, Cargo's locked dependencies already cached,
`dbus-daemon`, `gdbus` and GNOME Keyring available, run:

```sh
export CARGO_TARGET_DIR=/absolute/scratch/cargo-target
python3 -B scripts/qualify-linux-secret-service.py \
  --output /absolute/new/secret-service-evidence
```

Use `--daemon /absolute/path/gnome-keyring-daemon` and optionally
`--library-path /absolute/private/library-directory` for a previously prepared
private installation. The script does not install packages, change `HOME` or
select the login-session bus. It starts a new private bus, verifies the bus and
Secret Service peer PIDs, and gives its foreground daemon private XDG paths.
Only random credential IDs and synthetic values are used. The separate
`secret-store-qa` binary imports the production credential files directly and
refuses operations without its fixture's private bus, runtime and nonce.

The original ignored roundtrip test runs first. Additional phases replace a
synthetic key, lock the private collection, require read/write/delete storage
errors, and check that no profile fallback directory appeared. Only the owned
daemon is then restarted with the same private keyring and password. The original
value must survive before deletion and repeated deletion are checked. This
qualifies headless locked-store failure and preservation across that restart;
it does not qualify desktop password dialogs, every Secret Service implementation,
Flatpak or the full application.

Compilation uses one job, offline locked inputs, a 3-GiB memory preflight and a
1-GiB stop. Each compilation is bounded to 15 minutes, the original native test
to 60 seconds and each additional credential phase to 20 seconds. A parent can
also supply `--stop-file /absolute/interlock` to stop owned work. The controller
terminates its process groups and removes its temporary keyring before success.
Evidence retains source/binary hashes and logs, never the keyring or its random
password. A cleanup failure makes the result fail. Missing system-prompt warnings
are expected during the intentionally locked headless phase and remain in the logs.

The committed probe/controller at `ef2cc9a` passed under WSL Ubuntu 26.04 with
GNOME Keyring 50.0. The original native test passed once and all four additional
phases passed, with source/binary integrity, committed-input hashes and cleanup
verified. Separate full-core checks passed 129 Linux tests (one opt-in ignored)
and 132 Windows tests, plus strict all-target Clippy on both. Earlier probe
compilation, controller-unlock and missing-Node fixture failures are preserved.
Evidence: `.dev/linux-secret-service-20261114/qualification.json`, SHA-256
`39ab8a9bfb750dc406dc5305ca8cff62e1c0a93c50071e09d0aa46e3200052ec`.

## Current file-inclusion policy at RAG source reads

`4b75693` includes the current content categories in `RagPolicy`, supplied from
the saved application settings. New retrieval and saved-source revalidation
check the authorized file against those categories before hashing its original
bytes. Unknown attributes or unreadable repository rules deny that source. The
default include-all path skips the new fact reader; filename visibility remains
independent of content permission.

Policy stamps are checked before sending and after generation for local and
external answers. A category change during generation cannot publish a complete
answer at that check, even if the settings revision has not advanced. A separate
post-persistence category-revocation race check remains pending. The original regression
failed because the post-generation stamp check applied only to external evidence.

Windows core tests passed 144 cases and strict all-target Clippy. Linux core tests
passed 142 cases with one native opt-in ignored; its helper passed three cases
with one native opt-in ignored. Linux strict all-target Clippy also passed. The
actual Windows app library passed all 564 ordinary tests, with 19 opt-ins ignored.
Its additional cached-index fixtures verify that rejected source-code, hidden
and repository-ignored files do not reach the original-file hasher, and allowed
evidence below denied candidates can fill the requested result. Linux core QA
does not include this app adapter.

At `4b75693`, this was one enforcement boundary, with filtering limited to the
existing retrieval pool. The following change moves candidate admission earlier.
Admission into content extraction, filename consumers and settings controls remain
separate work. No new native UI, model/provider, macOS
or deployed-service qualification is implied. Captured results and failed-run
evidence have qualification SHA-256
`6caca55f3499e5a5214d37bf567ac2c1f0af41ad219971ea8ec3f31639a61172`.

## Admission before retrieval limits

`a744009` passes the current RAG policy into the actual keyword and vector
repositories. Keyword admission runs before full chunks occupy result slots;
vector admission runs while the nearest-neighbor query expands its candidate
batch. A regression retrieves the permitted file behind 120 denied source-code
files, exceeding the service's 100-hit preview. The vector fixture uses actual
SQLite/Lance storage with synthetic two-dimensional vectors: the first 32
neighbors are denied, a larger existing budget finds the permitted source, and
the 32-candidate cap correctly retains an empty, explicitly limited result.

The default APIs delegate to allow-all admission, while canonical-root and
fingerprint checks remain mandatory. Candidate budgets and global hybrid branch
limits are unchanged. Keyword results recheck admission before returning text.
Internal app-cache exclusion also happens before result slots are filled. These
callbacks narrow source access and cannot authorize files outside content scopes.

The actual Windows app library passed 566 tests with 19 opt-ins ignored, covering
the RAG adapter, content service and vector store. Linux platform QA passed 240
tests with 13 opt-ins ignored, including the imported production ContentStore.
Platform QA strict all-target Clippy passed on Windows and Linux; it does not
include the vector store or Tauri adapters. The initial Windows vector test
compared extended and normalized path spellings; it now checks the stored source
chunk ID and path, retaining the same budget and backfill assertions.

Application settings still need to supply these categories to the content
collector and ordinary search paths. Filename policy, settings controls, native
UI/restart, fresh model inference and macOS qualification remain pending. Evidence
qualification SHA-256:
`3eda559829e1cafc13741194379cedf68ab2023b958a56adce8db186aa797435`.

## Policy changes during answer persistence

The post-generation check now has a second check after SQL completion. If current
content categories differ from those used by the successful provider attempt, or
the current policy cannot be read, the service withdraws only that request's
completion before returning an error. The store checks both conversation and
request ownership and requires a complete pair. One SQLite transaction marks
both turns and the request failed, clears the assistant body in the search column
and JSON payload, and updates the conversation timestamp. Pending/failed requests
and pairs belonging to another conversation cannot be rewritten by this method.
Previously completed history remains intact.

The actual service regression first failed because an after-save category change
still returned a successful answer. It now checks failure state, empty body,
search/export rejection, request ownership, repeated withdrawal and preservation
of another completed conversation. Existing external-server after-save hiding
regressions still pass. Windows RAG QA passed 145 tests and Linux passed 143
library tests plus three helper tests, with two Linux native opt-ins ignored.
Strict all-target RAG Clippy passed on both hosts. The actual Windows app-library
suite passed 567 tests with 19 opt-ins ignored.

These checks use an owned SQLite profile and a synthetic local provider. They
qualify the post-save hook and transactional withdrawal, rather than an atomic
transaction spanning settings and every concurrent reader, native GUI/restart,
power loss, storage failures or real model inference. Full content/filename policy
application and controls remain separate work. Qualification SHA-256:
`2ccd5561d226fc62858725dfbcadb8077230f147611b50d4b9fb44767662490f`.

## Application content-inclusion policy

The application now supplies saved content categories on startup, settings
restoration and save. A scan captures them in its generation; a configuration
change cancels the old scan, searches and password retries. Supported files pass
admission before extraction, and extracted text is checked again under the
configuration lock before storage. Excluded directories are pruned while explicit
nested content roots retain their own attribute ancestry. Filename-only roots
cannot reset that ancestry. Default inclusion skips the additional fact reader,
and scoped default searches avoid extra all-folder canonicalization.

Keyword/vector results apply the current service policy before filling slots.
Search snapshots, chunk lookup and document resolution also apply it, including
for already-cached text. A policy revision change during a query invalidates the
response. Password retries check policy before extraction and require the same
policy revision before hashing/publication. This controls reads and result
visibility; it does not promise physical erasure of previously cached text.

Settings get/save commands now run blocking work off the UI thread. An operation
mutex orders saves/restoration through event emission. The settings/config-error
locks are released before shortcut registration or window work, because native
shortcut calls dispatch to the main thread and wait. The state/disk commit shares
the existing settings lock with updater handoff persistence. Command names,
arguments, responses and revision-conflict checks are unchanged.

The actual Windows app-library suite passed 570 tests, with 19 explicit opt-ins
ignored. New fixtures verify exclusion before the extraction dispatcher, filtering
of cached results, search/chunk/snapshot/open paths, explicit nested roots, legacy
OCR configuration, old-work cancellation and password-retry revocation. The
ordinary app-library compile also passed, retaining nine unused-code warnings.
An earlier lifetime compile error and memory/disk preflight stops are retained.
The final functional suite uses 64 code-generation units for the application
package only, retaining optimization level 1, assertions and overflow checks;
profile manifests/dependencies are unchanged and this is not a performance run.

This harness still excludes the content/index/vector application adapters. The
new results above come from the full Windows app, not this small QA crate. A
fresh native settings/shortcut/multi-window/restart run and Linux/macOS builds
remain pending, as do filename inclusion and settings controls. Qualification
SHA-256:
`1acb284a934e2a6c9d6f6ffbeb47438871ceb47f407067256c5367d60a5a08ee`.

## Persisted source identity before original reads

The application's content-source resolver now requires a current chunk with the
same ID, fingerprint and canonical path before requesting an original-file hash.
A stored conversation reference alone is insufficient. Unknown IDs, mismatched
fingerprints/paths and chunks removed by the content service's internal
exclusions remain `Changed` with no text and no hash request. Existing `Deleted`
and `OutOfScope` classifications keep their precedence. Valid sources still
require a fresh original hash and a post-hash canonical-path check; duplicate
references share one hash per validation pass.

The actual Windows application regression first failed because an unknown ID
still requested a hash, then passed with the corrected order. The complete app
library suite passed 576 tests, with 19 explicit opt-ins ignored. This small QA
crate does not compile the application content-source adapter. The functional
suite retains opt1/assertions/overflow with command-local application codegen64;
it is not a new GUI, inference, Linux/macOS app or performance qualification.

An additional whole-app `clippy --lib --tests -- -D clippy::all` check failed on
13 diagnostic lines verified unchanged from the source base. Those diagnostics
and existing vendor/compiler warnings are preserved; no suppression was added.
This result is not a clean whole-app lint qualification. Evidence:
`C:/Users/solso/AppData/Local/Temp/prism-q18-source-identity-20261114-sjik2ku3/qualification.json`,
SHA-256 `8f4e43fb580d478f291a3f3c0794ad77eaa89986c15c5e3d82382c621cde7199`.

The later [application Clippy cleanup](../platform-qa/README.md#application-clippy-cleanup)
resolved those diagnostics and the subsequently exposed fixture lints. The
historical failure above remains separate from the newer passing result.
