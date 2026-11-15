# Filename preparation checks

This crate imports the production filename index, compact storage, search patterns
and private metadata-capture reader without linking Tauri, a WebView or vector
runtime. The settings type is only a DTO fixture. No service is started, upgraded
or contacted by the benchmark. Captures must stay private.

The crate also imports the production Q18 file-category policy. Its tests cover
independent filename/content defaults, unknown facts, and filename-derived code
and archive categories. Default inclusion needs no facts; exclusions cannot turn
missing metadata into a confirmed negative. Hidden/system facts must include
ancestors, and repository-ignore facts must come from hierarchical rules.
Language extensions and named build scripts identify code; archive extensions
identify archives, while ZIP-based documents such as DOCX, HWPX and EPUB remain
documents. This performs no content sniffing or archive extraction.

Actual Windows app-library tests separately cover settings-file round trips,
content extraction/publication, keyword/vector admission, snapshots/chunks/open,
password retry and RAG policy checks. This filename fixture cannot prove those
app adapters. Filename walker/service admission, live queries, snapshots and
result resolution now apply the independently saved filename categories.
Restricted candidates are checked before ranking limits; snapshots and original
file actions use fresh facts, including changed repository rules. Explicit
nested scopes remain independent when a parent is pruned. Category changes
revoke provisional rows and invalidate an older collection generation.

The Windows snapshot consumer requests attributed records when hidden/system
facts are needed. An explicit unsupported response selects the legacy transfer;
unknown attributes require fresh facts. Mixed transfer modes are rejected.
Filtering happens before private preparation and publication. Default inclusion
keeps the legacy transfer and skips filesystem fact work. No flags are retained
in compact catalog nodes. Actual deployed-service interoperability still needs
separate qualification.

Settings expose separate filename and content groups in Korean, English,
Simplified Chinese, Traditional Chinese and Japanese. The controls preserve
folder-level content and external AI permissions. Their default is to include
all categories; excluding an archive does not exclude ZIP-based documents.

The shared filesystem fact reader now resolves restricted categories from
native metadata and repository-local rules. Hidden/system attribute ancestry
starts at the selected root; known OS system locations are separate from user
homes, document portals and temporary documents. Volume-root implementation
flags do not hide an entire drive. Default inclusion skips name classification
and filesystem queries. These predicates narrow existing scope authorization.

Repository matching uses pinned `ignore` 0.4.33 with case-sensitive patterns
from nested `.gitignore` files and common-dir `info/exclude`, including Git
worktree pointers. Parent-directory exclusions cannot be reopened by a child
rule. Global Git configuration and the tracking index are not consulted.
Production checks do not execute Git. The opt-in oracle compares 16 paths with
`git check-ignore --no-index -c core.ignorecase=false` in an owned repository,
with system/global configuration and user templates disabled.

Rule reads reject links/reparse points and multi-link files. Each file has a
64-KiB/1,024-line worker budget; each path has at most 16 active nonempty rule
files and 256 KiB of rule text. Gitdir/commondir pointers have a separate 4-KiB
bound. Directory and missing-rule caches each retain at most 4,096 keys/4 MiB;
compiled rules retain at most 16 files. Errors remain unavailable evidence.
Caches are not live permission state: recreate a reader for each validation pass.

The Windows metadata-only `check_windows_snapshot` API accepts complete own
attributes from a caller-validated snapshot. Known flags avoid a leaf stat;
unknown attributes retain the fresh-stat fallback when facts are required.
Negative own flags do not override hidden/system ancestors, repository rules or
scope/link checks. Reparse entries are rejected. NTFS ARCHIVE/COMPRESSED flags
do not identify archive filenames. Use fresh `check()` plus scope authorization
for original-file reads/actions; snapshot facts are not access authority.

The selected volume entry itself is exempt from implementation hidden/system
flags, just as ancestor roots already were. A real Windows root-metadata
regression first failed with `Exclude(Hidden)` and now passes, including the
actual content admission helper. These tests never enumerate a user's drive.
An owned 4,098-directory fixture also verifies exclusions after the existing
bounded directory/missing-rule caches reset; their implementation is unchanged.

Current filename checks passed 112 tests on Windows (seven ignored) and 105 on
Linux (four ignored), with strict all-target Clippy on both. The opt-in Git
oracle passed separately in the earlier fact-reader qualification. Actual
Windows app-library tests passed 586 (19 opt-ins ignored), using command-local
app codegen64 with opt1/assertions/overflow checks retained. App Clippy passed
`-D clippy::all`; existing compiler/vendor warnings remain, so this is not an
app-wide `-D warnings` result. No manifest/profile changes or timing claim.

Production-bundle browser tests passed 38 cases, including independent settings
persistence, cancellation, five languages and narrow-screen control bounds.
After adding the minimum-size cases, the focused inclusion suite passed 11:
desktop persistence/cancellation plus all five languages at 390×844 and 760×540.
The latter matches the configured minimum main-window dimensions; control bounds
and Save/Cancel visibility passed and all ten switches appeared in the five
760×540 screenshots. This is browser-viewport evidence, not an OS resize test.
Both Windows Computer Use paths reported an unavailable native pipe, so actual
minimum-window resizing remains unverified. Neither path sent resizing input.
Some first headless Chromium captures omit switch paint despite passing
interaction/layout checks; those captures remain an open visual limitation,
not a CSS fix. A freshly built bundled Windows app separately passed
`tests/native/file-policy.mjs`: real settings-file writes, process restart,
keyboard cancellation, collection, file/rule watcher updates and rediscovery.
Its desktop WebView screenshots show all ten controls in all five languages.

`tests/native/content-policy.mjs` separately exercises the same controls through
actual content extraction and SQLite keyword search. After an explicit folder
content grant, it finds six generated documents, including a real Windows
SYSTEM-attribute file and an ordinary child of a SYSTEM-attribute folder.
Filename SYSTEM exclusion and re-inclusion hide and restore both paths while
content access stays unchanged. Content exclusions hide code, hidden descendants,
repository-ignored files and system files while filename settings stay
independent. Previously issued chunk IDs are rejected too. The regression
restarts the app, re-includes the documents, then revokes folder content access
and checks that all content/chunk access disappears. Original bytes and the
owned fixture's original attributes are preserved; no model or provider runs.

The native test requires `PRISM_TEST_EXECUTABLE` and its known SHA-256 in
`PRISM_TEST_APP_SHA256`, then runs with `node tests/native/file-policy.mjs`.
All three policy fixtures copy that executable into their own temporary app
directory and create its portable marker and `PrismData` before launch. They
verify the original and copied hashes, runtime build flavor, portable mode,
scope identity and disabled clipboard history. `PRISM_TEST_EXPECT_RELEASE=0`
(the default) requires a debug build and checks its synthetic clipboard fixture.
Set `PRISM_TEST_EXPECT_RELEASE=1` explicitly for a release candidate; that path
strips debug overrides and never calls debug-only fixture commands. Portable
isolation applies to both flavors, including a mistakenly supplied release
binary. The tests never change an installed service or user clipboard.

The portable-profile safety suite also exercises fixed CDP-port reuse with real
loopback listeners. A transient holder must release the port before the runner
reaches process creation; a persistent holder must produce `EADDRINUSE` after the
bounded deadline without a connection or process creation. Process creation is
intercepted, so these checks never start Prism or a WebView. With identical test
input, the pre-retry harness passes six checks and fails these two, while
`bef809c` passes all eight. This verifies the reservation guard, not an actual
WebView child lifetime or the native policy flows.

The content regression accepts the same executable/hash variables and runs
with `node tests/native/content-policy.mjs`.

`node tests/native/container-policy.mjs` accepts those same executable/hash
variables. It copies the pinned public plaintext DOCX and generated EPUB fixture,
uses the EPUB package as an actual ZIP archive, and exports a manual HWPX through
the running app without a provider. It checks that excluding archives hides the
ZIP filename while retaining DOCX/HWPX/EPUB filenames. A separate UI content
grant must make the actual DOCX, HWPX and Markdown searchable while the archive
settings remain excluded. The test restarts the app, re-enables archive filenames,
and revokes content access, including access through previously issued chunk IDs.
It preserves all original fixture bytes and the executable hash. EPUB is checked
only for filename classification; this does not claim EPUB or ZIP content
extraction, document layout fidelity, installed NTFS or another platform.
On 2026-11-15, all three portable release fixtures passed locally against the
actual `qa-windows-app` from [CI 43675760633 at
6409366](https://github.com/gaon12/prism/actions/runs/43675760633), using the
`bef809c` harness. The app SHA-256 was
`621e17161ac760684da46bdfd58082d4488c2729f6714ab3956166aba8cece21`;
the downloaded artifact ZIP matched GitHub's digest before extraction. That
hosted run had passed required 1.7B inference but failed the policy fixture's
restart because a WebView child still held the fixed CDP port. The updated
harness reserves the port before process creation and waits at most 20 seconds
for an occupied explicit port. The local run reused one explicit loopback port
across the three fixtures and their real process restarts.

Filename collection, watcher updates, independent content restrictions and stale
chunk rejection all passed, as did ZIP exclusion with DOCX/HWPX/EPUB filename
retention, real DOCX/HWPX extraction and content revocation. Screenshots from
all five languages and the filename/content/container states were inspected;
the controls and Save/Cancel buttons were visible. Windows WebView2 reported
`157.0.4341.46`; content checks used a 1280×840 viewport at scale 1.25.

The sequential run took 34.234 seconds, with 4.33 GiB minimum host availability
and no resource/deadline stop. The owned Job Object had no remaining processes
at cleanup. The original executable and all nine frozen runner/fixture inputs
retained their hashes. Evidence is preserved outside the system Temp directory
at `C:/Users/solso/prism-agent-work-20261115/native-policy-640-local1/`, including
per-fixture logs, screenshots and `result.json` (SHA-256
`ca206f0c0e8415a3d4e35a156edc941c33c8ce8b89e977fb6837ad6b6a3a912f`).
This was a functional Windows ordinary-filesystem check, without a CPU or
512 MiB limit. It does not qualify deployed NTFS transfer/fallback, another
platform, native minimum-window resizing, model inference, or app performance.
The candidate predates vendor relocation; identical product inputs and vendor
tree contents were checked. The following run covers the relocated app's
functionality; the later hosted comparison below covers cache reuse separately.

The subsequently produced vendor-relocated Windows app from [CI 43679179881 at
7ba77a2](https://github.com/gaon12/prism/actions/runs/43679179881) also passed all
three local policy fixtures with the same `bef809c` harness. Its artifact ZIP
SHA-256 matched GitHub's `a92dd4c699dfb12abdd003dbad330eae43576c46ee458cfa09c210f84070a9e8`
digest; the extracted executable SHA-256 was
`512efd0c91dfe8ddb9b5b37b1be80d1aaf97becc6ef1f56bf3e71dd666a46401`.
The run took 34.610 seconds, with 4.79 GiB minimum host availability, unchanged
inputs/executable, no resource stop and no remaining owned Job processes.
Evidence is `C:/Users/solso/prism-agent-work-20261115/native-policy-7ba-local1/result.json`
(SHA-256 `e2bd343d3ab01343bb9b79cf8379e07b7105f5cb5782b77e3f46b4e8b731da9b`).
Terminal logs from that source also confirmed actual release-library results
of Linux x64 578 passed/16 opt-ins ignored, Linux ARM64 578/13 and macOS ARM64
577/17. Windows workflow checks were still running at that inspection time.

Both that run and [CI 43683382725 at
0b6910d](https://github.com/gaon12/prism/actions/runs/43683382725) subsequently
completed with all seven build/check jobs successful; draft publication was
skipped. Terminal Windows logs confirm the document/export gate, official
embedding transfer/cancellation (394 and 389 progress frames respectively),
required actual 1.7B inference through reopening the document and answering,
and all three actual release policy fixtures. The latter run used executable
SHA-256 `888563849d52d0a6f7ba0a168e80ef95fbf6c130c0ad317300e6dd53d83cead1`.
Both actual Windows release-library suites passed 586 tests with 19 explicit
opt-ins ignored. The later run includes the eight portable-profile guard tests.

The first Windows run reported no Rust cache and took 57m 22s for its first
release compilation. The following run restored the vendor-tree-specific
`v0-rust-vendor-be8a5ac0e08e-build-Windows_NT-x64-1ffb72bd-0fc0b59c` cache and
reported 5m 08s for that compilation. Production app/vendor inputs did not change
between those sources; their differences are harness, tests and documentation.
This is one observed cold/warm hosted pair, not a repeated benchmark or a
guaranteed build time. It concerns compiler work, not app startup performance.
The raw terminal summaries and per-fixture JSON are retained in
`C:/Users/solso/prism-agent-work-20261115/release-followup-20261115.json`.
Native other-platform policy UI, deployed NTFS transfer/fallback, actual native
minimum-window rendering and the low-end whole-app performance target remain
separate qualifications.

Earlier debug evidence was recorded at
`C:/Users/solso/AppData/Local/Temp/prism-q18-filename-20261114-1c455odk/qualification.json`.
The later SYSTEM-folder and minimum-viewport checks are recorded separately in
`C:/Users/solso/AppData/Local/Temp/prism-q18-filename-20261114-1c455odk/system-and-layout-qualification.json`.
That old Temp worktree and its local evidence disappeared on 2026-11-15; the
paths describe historical records, not currently available local artifacts.

`tests/native/dictionary-download.mjs` is an independent opt-in Windows portable
regression for real dictionary download cancellation. In addition to the same
executable/hash/build variables, it requires `PRISM_TEST_DICTIONARY_NETWORK=1`.
It imports one generated CC0 seed entry, starts the product's fixed official
Kaikki Korean HTTPS download, and waits for actual bytes in a distinct owned
staging file before cancelling. It checks the terminal cancellation, seed-pack
lookup, pre-existing legacy/download staging files and cleanup of the new stage.
No response is replaced and no full corpus import is required. External endpoint
failure is retained as a failed run rather than replaced with a synthetic pass.

On 2026-11-15, the actual `6409366` Windows release candidate above passed this
fixture at `e4e6ab2`: 185,630 official HTTPS bytes were observed both in the
product's progress state and its distinct staging file. Cancellation returned
the typed `cancelled` result in 110 ms. The generated seed pack and both existing
files retained their contents; the new stage was removed. The 5.219-second run
had 4.96 GiB minimum host availability, unchanged executable/runner hashes, no
resource stop and no remaining owned Job processes. Evidence is
`C:/Users/solso/prism-agent-work-20261115/dictionary-download-640-local2/result.json`
(SHA-256 `62560e2e6a1831a5e2488c35398cb1c556c23699caea9ee3cabd4c40a198b438`).
The first run remains recorded as failed: Playwright converted a typed IPC
rejection into an `Error`, losing its code. The fixture now serializes the
rejection inside the app page and retains the cancellation-code assertion;
product code is unchanged. This is real transfer/cancellation evidence, not a
complete corpus download/import, definition-quality, symlink or other-platform
qualification.

Set `PRISM_TEST_DICTIONARY_COMPLETE=1` as a second explicit opt-in to continue
after cancellation with a complete official Korean corpus download/import.
The fixture limits that transfer to 512 MiB, retains failures, checks actual
offline lookup and rendered attribution, then restarts the real app and checks
the imported pack and lookup again. The generated seed and both earlier staging
files must still be unchanged. It does not imply other-language corpus coverage
or general definition quality.

The first actual completion run reproduced a product failure in the `7ba77a2`
release app: all 207,649,961 official bytes downloaded, but import failed at the
old 1 MiB record limit. The captured corpus (SHA-256
`c160b1dadd49b14109c2c1a29a616cc44064a738f91d2b98c1cdf457b90fde7f`)
has 64,457 records; nine exceed 1 MiB and the largest is 3,691,949 bytes, mostly
unused sense metadata. The failed native run is retained at
`C:/Users/solso/prism-agent-work-20261115/dictionary-complete-7ba-local1/result.json`.

The importer now bounds a record at 8 MiB. Records up to 1 MiB retain their
existing parser. Larger records project known dictionary fields while parsing,
without constructing unrelated metadata trees. Selected arrays retain only the
existing 40 nonempty meanings, 300 aliases and four examples; the existing
stored string limits and transaction/cancellation checks still apply. The rest
of the JSON is validated rather than silently skipped as an accepted record.
Malformed, cancelled and over-limit replacements preserve the previous pack.

The old parser failed the new large-record regression. The fixed production
store passed seven tests and strict all-target Clippy in a lean harness with
the app's exact serde, serde_json, rusqlite, URL, normalization and SHA-256
dependency versions. The tests compare small/large selected-field results and
cover malformed trailing metadata, the 8 MiB read bound and mid-read cancellation.
The same captured official corpus then imported 76,689 meanings and returned
five definitions for the tested word after reopening the store, in 10.477 seconds
excluding build time. Frozen source/input checks and resource guards passed;
owned process handles were signaled after Job cleanup. Results are under
`C:/Users/solso/prism-agent-work-20261115/dictionary-rust-verified-{green,clippy,corpus}/`.
This proves the production importer with that corpus, not a full Tauri app,
rendered lookup, network transfer after the fix or a 512 MiB whole-app budget.
The updated completion fixture still needs a newly built app for that final
native qualification; the earlier release PE is deliberately not treated as
containing this source fix.

All seven build/check jobs in [release run
43673534172](https://github.com/gaon12/prism/actions/runs/43673534172) at `4054072`
passed. The actual release-library results were:

| Platform    | Passed | Explicit opt-ins ignored |
| ----------- | -----: | -----------------------: |
| Windows x64 |    586 |                       19 |
| macOS ARM64 |    577 |                       17 |
| Linux x64   |    578 |                       16 |
| Linux ARM64 |    578 |                       13 |

These include the policy/collection regressions. Windows also passed the actual
document/export gate, official embedding transfer/cancellation (394 progress
frames), and required 1.7B fixture inference through reopening the generated
document and answering. That run predates the portable policy qualification
above and vendor relocation. Native macOS/Linux policy UI, deployed NTFS transfer and
fallback, narrow native rendering and new app memory/performance qualification
remain pending.

`tests/e2e/search-policy-cache.spec.ts` uses the actual React UI with synthetic
Tauri IPC and a frozen index revision. It covers displayed and still-pending
filename/content responses when inclusion settings change, so cache invalidation
cannot depend on a later indexing event. The [hosted Client run at
699bb90](https://github.com/gaon12/prism/actions/runs/43674747267) passed current
format/type checks, 154 unit assertions and all 47 production browser cases,
including these four regressions. This is a browser regression with synthetic
IPC, not native backend evidence. The [subsequent run at
93733ae](https://github.com/gaon12/prism/actions/runs/43675095547) also passed all
47 cases, with no skips, retries or unexpected results. Its preserved per-case
JSON confirms all four cache cases passed without page/console errors or
compatibility notices. All four 1280×840 cache screenshots were inspected: only
the permitted filename or public excerpt remained. The downloaded artifact
matched its GitHub SHA-256 digest. The hook change requires a new frontend
bundle; the older frozen debug executable above does not qualify this additional
behavior.

```sh
cargo build --manifest-path tools/filename-qa/Cargo.toml --locked --release -j1
cargo test --manifest-path tools/filename-qa/Cargo.toml --locked --release -j1 --lib -- --test-threads=1
cargo clippy --manifest-path tools/filename-qa/Cargo.toml --locked --release -j1 --all-targets -- -D warnings
```

The `prism-filename-qa` executable accepts `decode`, `baseline` or `stages`, followed
by a private PNROW001 capture path. It prints `waitingForStart`, then requires a
single `G` byte before opening the capture. A valid low-end measurement driver must
first assign that owned process to a Windows Job Object and independently read
back one-CPU affinity, both 512 MiB process/job commit limits and its process ID.
The executable additionally checks that its usable CPU count is one. The existing
[Job helper](../../tests/native/support/job-limit.ps1) exposes the limit readback.
EOF, another start byte or an invalid command fails without reading the capture.

`baseline` uses the production fused `Entry::new(raw(row), sweep)` loop and compact
insertion. `stages` times the same operations in separate bounded batches: raw
identity mapping, Entry search/projection construction, and insertion/drop.
It temporarily retains one extra Raw batch; it does not time each row. Batch size
is the protocol's `MAX_BATCH_RECORDS`, independent of the capture's original frame
boundaries. The private raw mapper is emitted verbatim from the current production
file at build time; its digest is included in every result.

Both preparation modes hash all stored display/key/search/projection strings,
name/directory flags, size, optional timestamp and sweep in storage order. Compare
their count and SHA-256, not only time. Verification runs after timed preparation
and its duration is reported separately. `decode` measures capture decoding and
dispatch/drop without building a catalog. Stage times are wall time; process CPU
and peak memory must be taken separately from the Job helper.

These are component measurements over a captured metadata distribution. They do
not establish live service/IPC latency, provisional-search contention, WebView
memory, cold MFT behavior, HDD performance or whole-app startup. Do not upload
capture data or print path values in logs.

Two additional modes, `reconcile-legacy` and `reconcile-seen`, use that same
fused insertion path and then measure the first whole-drive sweep's retention.
Every inserted entry is seen by sweep 1. The legacy mode calls the actual
`Entries::retain` and checks `is_seen` in its closure, after the method has
rebuilt the key. The current mode calls the actual `retain_seen_or`, which skips
key reconstruction for seen entries. Both modes hash all stored fields before
and after retention and reject any change in this all-seen catalog.

Compare the same release executable in a fixed legacy/seen/seen/legacy sequence,
using the same private capture and verified one-CPU, 512 MiB Job limits. Results
report retention wall time, unconverted thread CPU cycles, raw single-interval
`GetThreadTimes` CPU and the number of keys passed to the predicate. CPU accounting
can be coarse for a short retention phase. Build time excludes retention; the
before/after SHA work has separate clocks. Whole Job CPU includes all of them.
This isolates retention on an owned catalog without a catalog lock, GUI queries
or contention. It cannot establish the window's blocked duration. A regression
also checks equal retained fields and deletion of stale rows in both modes.

## Observed component baseline

On 2026-11-13, a fixed decode/baseline/stages/stages/baseline sequence replayed
2,185,112 private metadata rows in fresh one-CPU, 512 MiB process/job limits.
The capture and executable stayed unchanged. All four prepared catalogs had the
same complete text/projection/metadata hash and entry count; 99.8% of paths were
ASCII. The fused baseline took 4.523 and 4.570 seconds to decode and build, with
about 182–183 MiB peak job commit. Decode alone took 0.797 seconds.

| Stage (wall seconds)    | First separated run | Second separated run |
| ----------------------- | ------------------: | -------------------: |
| Raw identity mapping    |               0.213 |                0.216 |
| Entry construction      |               0.239 |                0.215 |
| Compact insertion/drop  |               3.472 |                3.399 |
| Decode and dispatch     |               0.669 |                0.673 |
| Complete decode + build |               4.592 |                4.503 |

Insertion accounted for about 89% of preparation time in this fixture. These
local release measurements used Rust 1.99.0 and the source at `207906d`. The same
crate's 75 ordinary tests and strict Clippy also passed with Rust 1.100.0 in
[Windows CI 43387985767](https://github.com/gaon12/prism/actions/runs/43387985767).

The input was a warm capture on an NVMe host with more than 512 MiB physical RAM,
not a live drive request or a physical low-end/HDD machine. The five cases lacked
GUI, IPC and provisional-query contention. They do not qualify the requested
whole-app 5–7-second startup. Likewise, the harsh profile's aggregate app/WebView
Job CPU must not be reported as Entry-construction CPU. Per-process CPU and
app-side waits need their own measurements.

## Explicit synthetic CPU profile

The optional `profile_cpu` input of `filename-qa.yml` runs
`python -B scripts/profile-filename-cpu.py --output <new-evidence-directory>`.
Locally the output must be inside `.dev`; the workflow uses `RUNNER_TEMP`.
The driver builds the current release test executable and runs the exact ignored
`synthetic_preparation_cpu_stages` test in a fresh one-CPU, 512 MiB process/job
limit, with kernel limit readback before releasing its start gate.

The fixed sequence is fused/stages/stages/fused, each with 2,200,000 rows from the
existing synthetic generator (6.25% CJK names, 10,007 directories). The actual
protocol codec, private raw mapper, Entry constructor, compact insertion and
`publish_provisional` implementation are called directly. Input generation and
encoding, complete stored-field verification, setup and clock calibration are
reported separately. A streaming, noncryptographic checksum must match before
and after publication and across all four cases.

Each requested stage reports raw thread user/kernel CPU from `GetThreadTimes`,
wall time and `QueryThreadCycleTime` cycles. Cycles are the primary stage CPU
measure and are never converted to time. Short-batch CPU-time differences can
report zero and need not sum to the whole-worker CPU time. The driver reports
both sums explicitly and marks stage CPU milliseconds unsuitable for attribution.
Instrumentation is
per bounded protocol batch, and the fused cases provide a control for separating
raw mapping from Entry construction. The driver preserves source/executable
hashes, compiler version, full logs, job CPU and peak commit. It requires 3 GiB
host memory before starting and stops owned processes below 1 GiB available.

This synthetic component profile has no IPC, GUI, concurrent searches or live
service. Its CPU costs and hosted-runner hardware must not be substituted for
whole-app startup on the target PC. The uploaded evidence contains synthetic
rows only; the private capture is neither read nor uploaded.

On 2026-11-13, [Windows CI 43395193150](https://github.com/gaon12/prism/actions/runs/43395193150)
at `97cf663` passed all four synthetic cases, 76 ordinary tests and strict Clippy.
All cases stored 2,200,000 entries, preserved the same full-field checksum across
publication, and peaked at 172.7–173.3 MiB job commit.

| Stage                        | First separated run (billion cycles) | Second separated run (billion cycles) |
| ---------------------------- | -----------------------------------: | ------------------------------------: |
| Protocol decode              |                                1.008 |                                 1.017 |
| Raw identity mapping         |                                0.633 |                                 0.632 |
| Entry construction           |                                1.560 |                                 1.549 |
| Compact insertion/drop       |                                7.211 |                                 7.079 |
| Real provisional publication |                             0.000078 |                              0.000104 |

Insertion used 76.4–76.7% of preparation cycles (raw + Entry + insertion), while
Entry construction used 16.6–16.7%. This fixture therefore points first to compact
insertion, rather than intermediate Entry allocation. The sum of all measured
phase cycles covered more than 99.94% of whole-worker cycles. By contrast, summed
short-batch `GetThreadTimes` differences covered only 52.7–58.2% of whole-worker
CPU time and are unqualified for stage attribution. Their raw values remain in
the evidence. Whole-worker CPU was 5.58–5.72 seconds, including synthetic input
generation and full verification, so it is not a catalog startup time.

The earlier run `43394358771` showed the same preparation cycle shares on another
hosted runner. Its different absolute timings are not a before/after product
performance comparison. The product preparation code was unchanged. CPU cycles
remain in their native unit, following the
[Windows API guidance](https://learn.microsoft.com/en-us/windows/win32/api/realtimeapiset/nf-realtimeapiset-querythreadcycletime).

## Same-capture check of temporary allocation removal

The compact insertion change at `b3f041d` borrows the reused directory's search
text and compares an ASCII name's lowercase bytes without a temporary string.
Regression tests preserve case-sensitive Unix keys and projections added to an
existing shared directory. Its [CI 43396679430](https://github.com/gaon12/prism/actions/runs/43396679430)
passed 78 tests and strict Clippy; the [baseline CI 43396617065](https://github.com/gaon12/prism/actions/runs/43396617065)
at `057bf8c` passed 76 tests and strict Clippy.

The two CI-built release executables used the same Rust 1.100.0 compiler and
differed in only `filename_compact.rs`. Their artifact digests, executable hashes
and source commits were verified before replaying the same 2,185,112-row capture
locally in a fixed A/B/B/A sequence, each in fresh one-CPU, 512 MiB process/job
limits. A is the baseline; B removes the two temporary allocations.

| Case      | Whole Job CPU (seconds) | Decode + build (wall seconds) | Insertion (wall seconds) |
| --------- | ----------------------: | ----------------------------: | -----------------------: |
| A, first  |                   4.827 |                         5.090 |                    3.923 |
| B, first  |                   4.656 |                         4.529 |                    3.395 |
| B, second |                   4.984 |                         4.436 |                    3.330 |
| A, last   |                   4.780 |                         4.580 |                    3.475 |

Whole Job CPU includes decoding, preparation, complete stored-field SHA-256
verification and teardown. Its average was 4.8035 seconds for A and 4.8200 seconds
for B (+0.34%), with overlapping ranges: this check establishes **no CPU gain**.
Wall averages were lower, but the first A was also slower than the last A; those
observations do not qualify a startup performance improvement.

All four catalogs retained the same complete stored-field SHA-256
`2050731a23fb94bb1ba25399970e97a12beed3cf1533618712447fc9794f2b2c`.
Peak job commit was 181.8–182.5 MiB, input/executable/controller hashes stayed
unchanged, and no resource or deadline guard fired. This preserves allocation
removal and semantic equivalence evidence, while the whole-app 5–7-second target
still needs its separate measurement.

Later that day, an idle Hancom MCP server owned by the measuring Codex session
was found consuming one full CPU core and was stopped. The earlier local capture
replays did not isolate that background load, so their timings remain
observations, without a clean-host performance qualification. Stored-field
equivalence is unaffected. The hosted synthetic CPU profiles ran on a separate
machine.

One fixed follow-up A/B/B/A sequence after stopping that server reused the same
two CI executables and capture, preserving the original dataset:

| Follow-up case | Whole Job CPU (seconds) | Decode + build (wall seconds) | Insertion (wall seconds) |
| -------------- | ----------------------: | ----------------------------: | -----------------------: |
| A, first       |                   5.249 |                         4.582 |                    3.433 |
| B, first       |                   4.921 |                         4.524 |                    3.322 |
| B, second      |                   5.030 |                         4.428 |                    3.301 |
| A, last        |                   4.859 |                         4.581 |                    3.457 |

Average CPU was 5.0540 seconds for A and 4.9755 seconds for B (-1.55%), with
overlapping ranges. The two complete datasets therefore still do not establish
a reliable CPU gain. All four follow-up catalogs preserved the same SHA-256 and
entry count, all inputs/executables stayed unchanged, and no guard fired.
System CPU during each case, including the benchmark itself, ranged from 23% to
28%; this interactive host was not fully isolated. No favorable cases were
selected or substituted for the first dataset, and neither dataset qualifies
whole-app startup.

## Comparing directory keys without rebuilding paths

`65f3130` replaces the temporary whole-directory key in `find_dir` with an exact
comparison of stored path parts. Explicit keys keep their original spelling;
derived keys use the same ASCII lowercase and separator edits as reconstruction.
No per-entry storage is added. Differential tests cover case-sensitive keys,
Unicode, mixed separators, linked ancestors and freed directory-slot reuse.

The baseline at `1365068` and candidate were built with the same local Rust
1.99.0 compiler and differed only in `filename_compact.rs`. They passed 81 and
82 tests respectively (six explicit ignored tests each), both strict Clippy
checks and release builds. The same private 2,185,112-row capture was replayed
A/B/B/A in independently verified one-CPU, 512 MiB process/job limits:

| Case      | Whole Job CPU (seconds) | Decode + build (wall seconds) | Insertion (wall seconds) |
| --------- | ----------------------: | ----------------------------: | -----------------------: |
| A, first  |                   4.733 |                         4.322 |                    3.243 |
| B, first  |                   4.655 |                         4.106 |                    3.078 |
| B, second |                   4.562 |                         4.151 |                    3.060 |
| A, last   |                   4.671 |                         4.238 |                    3.204 |

Observed insertion averages were 3.2232→3.0693 seconds (-4.77%); whole Job CPU
was 4.7020→4.6085 seconds (-1.99%), including verification and teardown. These
are modest observations from one paired sequence on an interactive host, not
a repeated performance guarantee. System CPU usage during the cases was 22–24%,
including the benchmark. Absolute values must not be compared with the earlier
CI-built compiler's runs as a product speedup.

All four full stored-field hashes remained
`2050731a23fb94bb1ba25399970e97a12beed3cf1533618712447fc9794f2b2c`.
Peak job commit was 181.9–182.7 MiB, all source/control/capture/executable checks
remained unchanged, and no memory or deadline guard fired. This component result
does not qualify GUI startup, service transfer, IPC, HDD or the whole-app
5–7-second target. Evidence:
`.dev/filename-dir-key-20261114/qualification.json`, SHA-256
`79d2db7a3562a8e9a4f90691206c1d8bdd98b59305826442036a4580d324187b`.

## Isolating table growth cost

A private diagnostic overlay on `65f3130` timed `Table::rebuild` at function
boundaries and insertion at bounded-batch boundaries. No per-row clocks were
added. Two replays of the same capture in verified one-CPU/512 MiB limits each
recorded 35 table rebuilds and 3,538,920 existing IDs rehashed. Rebuild work took
1.852/1.924 wall seconds and 5.533/5.579 billion thread cycles, or 60.49%/60.36%
of the 9.148/9.243 billion insertion cycles. Cycle counts remain unconverted.

These totals combine the directory and entry tables, key reconstruction and
hashing, allocation and probing. They establish table growth as a substantial
insertion cost, not a separate cost for any one of those operations. The overlay
uses two clock reads per rebuild and insertion batch (1067 batches); clock
overhead is included. Its full-field SHA matched the uninstrumented binaries in
both runs, no guard fired, and all diagnostic inputs stayed unchanged. This is
component attribution, not an app-startup speedup. The diagnostic source changes
are preserved privately and were not added to the product. Evidence:
`.dev/filename-rebuild-profile-20261114/qualification.json`, SHA-256
`babbd429ff988df6aa6b375304461f7c5d32ff099da50ee494c78f31a5fbb28e`.

## Reusing computed hashes during table growth

`254a43f` caches the full keyed identity hash in each entry and directory record.
Growing or reserving either lookup table reads those hashes instead of rebuilding
and hashing whole paths. Lookup still compares exact keys to resolve collisions;
the cached hash is not a persistent identity or a substitute for key equality.
Lifecycle regressions cover growth, reserve, deletion, slot reuse, text-arena
compaction and catalog clearing. The records retain four-byte alignment, but
entries grow from 36 to 44 bytes and directories from 20 to 28 bytes.

The same local compiler and capture were used for a new isolated A/B/B/A pair,
with baseline `1cf7947`. Both release binaries passed strict Clippy; the baseline
passed 82 tests and the candidate 83, with six explicit ignored tests each.
Every case independently verified one-CPU affinity and both 512 MiB limits.

| Case      | Whole Job CPU (seconds) | Decode + build (wall seconds) | Insertion (wall seconds) | Peak job (MiB) |
| --------- | ----------------------: | ----------------------------: | -----------------------: | -------------: |
| A, first  |                   4.718 |                         4.134 |                    3.112 |          182.1 |
| B, first  |                   3.077 |                         2.460 |                    1.351 |          203.3 |
| B, second |                   3.108 |                         2.429 |                    1.343 |          203.6 |
| A, last   |                   4.686 |                         4.242 |                    3.164 |          181.7 |

Observed average insertion time fell from 3.1381 to 1.3472 seconds (-57.07%),
and whole Job CPU from 4.7020 to 3.0925 seconds (-34.23%). The full stored-field
SHA remained identical in all four cases, all source/control/capture/executable
hashes were unchanged, and no resource or deadline guard fired. The extra memory
is an explicit tradeoff: this component fits the limit; the actual app/WebView
combined-budget result follows below. This single paired sequence on
an interactive host does not qualify whole-app startup, service transfer, HDD or
the 5–7-second target. Evidence:
`.dev/filename-cached-hash-20261114/qualification.json`, SHA-256
`4e49d474d3e2fd9952fea1344baf87c6fddb26c3b8cc46be549cbb64c495c757`.

## Actual app and WebView budget check

The release app at `254a43f` was then compared with the preserved release app at
`290cac9`, in A/B/B/A order using unchanged `harsh-profile.mjs`. Each run owned a
fresh portable profile and constrained the app and its WebView processes to one
logical CPU and 512 MiB, with kernel limit readback. Software drawing was enabled
and verified, matching the app's low-memory drawing mode. C: had about 2.187
million live entries; fresh test profiles make that count rise slightly.

| Case      | Window (seconds) | First searchable (seconds) | Entire list ready (seconds) | Peak app/WebView job (MiB) |
| --------- | ---------------: | -------------------------: | --------------------------: | -------------------------: |
| A, first  |            2.261 |                      5.886 |                      13.688 |                      386.6 |
| B, first  |            1.596 |                      1.809 |                       6.785 |                      402.6 |
| B, second |            1.674 |                      1.897 |                       5.571 |                      389.5 |
| A, last   |            1.675 |                      1.734 |                       9.229 |                      383.8 |

Both candidate starts observed the 5–7-second window within this **app/WebView**
budget, with headroom below 512 MiB. Every case had one service snapshot and zero
overflow reconciliations. The installed NTFS service stayed at PID 5924 with
binary SHA-256 `d0b1fe5e90f59aa071bedea67fdd360832904092a8f703e72339ae71f04552ee`;
it was neither replaced nor restarted. The first baseline had a slower first
batch (5.666 s versus about 1.1–1.2 s in the other runs), so the startup averages
do not isolate the code change from cache/scheduling effects. All cases remain
in the evidence.

The LocalSystem service ran **outside** the CPU/memory job, and both physical
drives are NVMe SSDs. These two candidate starts are not a whole-PC 512 MiB,
single-CPU service, HDD, cold-cache or long-run latency qualification. Latest
cross-platform CI also remains pending. Evidence:
`.dev/harsh-cached-hash-20261114/qualification.json`, SHA-256
`b6f6e9282c70d0d4e729e12fe729eb73685873afbd1df65793393c6221dbc821`.

The same new app separately passed the unchanged actual 1.7B generation,
HWPX/reopen gate and PDF export of that generated revision, using normal host
resources. This is a regression check of document retrieval after the catalog
change; the optional generative model is not running in the 512 MiB filename
benchmark. Combined native evidence:
`.dev/native-cached-hash-254a43f-20261114/qualification.json`, SHA-256
`dd1df0d7d62da288c684f9ba8d58b7bc0318a3ddc61698fdc0d0072320758435`.

## Retention after the captured snapshot

[CI 43399104552](https://github.com/gaon12/prism/actions/runs/43399104552) at
`edc375d` passed 79 tests and strict Clippy, including mixed seen/stale retention
equivalence. Its single release executable then replayed the same 2,185,112-row
capture in legacy/seen/seen/legacy order on one CPU with 512 MiB process/job
limits, after stopping the identified busy MCP server.

| Retention path     | Wall milliseconds | Thread cycles (millions) | Keys passed to predicate |
| ------------------ | ----------------: | -----------------------: | -----------------------: |
| Legacy, first      |             271.4 |                    814.2 |                2,185,112 |
| Seen-first, first  |              6.95 |                     21.6 |                        0 |
| Seen-first, second |              6.62 |                     20.6 |                        0 |
| Legacy, last       |             276.4 |                    827.2 |                2,185,112 |

All entries were already seen by sweep 1. Before/after counts and complete
stored-field SHA-256 matched in all four cases. Retention averaged 273.9 versus
6.78 wall milliseconds, saving about 0.267 seconds in this fixture; measured
thread cycles fell by 97.4%. The fast path's raw CPU time readings were 15.625
and 0 milliseconds, so their accounting resolution cannot establish its cost.
Cycles remain unconverted. Peak job commit was 181.8–182.1 MiB, all input and
executable hashes stayed unchanged, and no resource/deadline guard fired.

This measures the actual retention methods on an owned catalog without a write
lock, window queries or service contention. The observed saving does not explain
the separate ten-second live-app delay. That delay still needs app profiling;
the whole-app startup target is not qualified by this component result.

## Replaying multiple watch paths

`prism-filename-qa watch capture.pnrows roots.private.json` measures the catalog
update after rewalking a fixed list of paths. The private JSON input is an array
of 1–64 path strings, limited to 64 KiB. Use the same immutable capture and path
file for both source revisions. As with the other replay modes, assign the
waiting process to the verified one-CPU, 512 MiB process/job limits before
sending its `G` start signal. No capture or path values are printed.

The build script extracts the watch-update branch from that build's actual
`src-tauri/src/index.rs`; `Entries::subtree` uses that build's actual compact
catalog. Comparing source revisions therefore measures their real branch and
catalog implementation rather than a manually reproduced legacy algorithm.
Record both source revisions, executable/input hashes and the emitted
`productionLoopSha256` and `normalizedPathsSha256` with the results.

All captured rows have already been seen by sweep 1, modelling paths just
rewalked after a snapshot. Input decoding, preparation, root-file parsing and
complete stored-field SHA-256 passes run outside the watch timer. The timed
branch includes subtree selection, stale-key collection and removal; it must
leave the all-seen catalog unchanged. A separate mixed seen/stale regression
checks actual removal, uncertain paths, overlapping roots in both orders and
an unrelated sibling. The executable reports wall time and raw current-thread
CPU/cycles without converting cycles to seconds.

This is a component replay without filesystem walks, a catalog lock, window
queries or service contention. It does not qualify app startup or explain the
frequency of watch events.

The 2026-11-13 fixed A/B/B/A replay used the same 2,185,112-row capture and
eight captured directories, selected by path SHA-256 among directories with
2–16 immediate children. This fixed list is not an observed watch-event trace.
Both isolated builds used the `af80a2a` harness and the same 207 source files,
except `index.rs` and `filename_compact.rs`: A took those two files from the
parent of `5ff473b`, and B took them from `5ff473b`. Local Rust 1.99.0 passed
80/81 tests respectively, six ignored in each, and strict Clippy for both.

The committed harness also passed [CI 43414049924](https://github.com/gaon12/prism/actions/runs/43414049924)
at `af80a2a` on Rust 1.100.0: 81 tests, six ignored, strict Clippy, formatting
and the release executable build. That hosted run did not replay the private
capture; it is separate from the local comparison below.

| Case      | Watch wall seconds | Raw thread CPU seconds | Thread cycles (billions) |
| --------- | -----------------: | ---------------------: | -----------------------: |
| A, first  |            2.51953 |               2.421875 |                    7.334 |
| B, first  |            0.35644 |               0.343750 |                    1.050 |
| B, second |            0.35856 |               0.359375 |                    1.055 |
| A, last   |            2.51598 |               2.328125 |                    7.299 |

Mean watch wall time fell from 2.51776 to 0.35750 seconds (85.8%) in this fixture.
Every replay preserved the full catalog SHA-256 and entry count. The source,
executables, capture and path file remained unchanged; all four processes had
the same one-CPU affinity and 512 MiB process/job commit limits before starting.
Peak job commit was 181.6–182.1 MiB, minimum host availability was 3.61 GiB,
and no resource or deadline guard fired. The four cases took 42.30 seconds in
total, including decoding, construction, SHA checks and setup outside the watch
timer. The measuring session's previously busy MCP PID 18976 was absent.

All four predeclared cases are retained. This interactive host was not isolated:
aggregate CPU busy during each whole case, including the benchmark itself,
ranged from 18.2% to 27.9%. The component replay demonstrates the lower cost for
this fixed eight-path input. It does not establish the live event count, an
eight-second GUI stall's full cause, or the whole-app 5–7 second startup target.
