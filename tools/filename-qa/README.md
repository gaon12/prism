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
