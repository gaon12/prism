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
