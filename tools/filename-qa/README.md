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
