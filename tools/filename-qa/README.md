# Filename preparation checks

This crate imports the production filename index, compact storage, search patterns
and private metadata-capture reader without linking Tauri, a WebView or vector
runtime. The settings type is only a DTO fixture. No service is started, upgraded
or contacted by the benchmark. Captures must stay private.

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
is an explicit tradeoff: this component fits the limit, but the actual app and
WebView need their own combined-budget result. This single paired sequence on
an interactive host does not qualify whole-app startup, service transfer, HDD or
the 5–7-second target. Evidence:
`.dev/filename-cached-hash-20261114/qualification.json`, SHA-256
`4e49d474d3e2fd9952fea1344baf87c6fddb26c3b8cc46be549cbb64c495c757`.

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
