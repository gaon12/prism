# Compact filename catalog QA

Runs the production `src-tauri/src/filename_compact.rs` module directly (it is
imported with `#[path]`, never copied), so its tests and memory benchmark work
without building the full app.

```sh
cargo test --manifest-path tools/catalog-qa/Cargo.toml --locked

# Live heap of one layout per process (counts, bytes and timings only; reads
# names and metadata, never file contents, and does not follow links).
cargo run --release --manifest-path tools/catalog-qa/Cargo.toml \
  --example catalog_memory -- compact --walk 'C:\'
cargo run --release --manifest-path tools/catalog-qa/Cargo.toml \
  --example catalog_memory -- old --synthetic 2000000
```

Measured on a real 2.0-million-entry Windows system drive (2026-11-12):

| Layout                               | Live heap | Per entry | Build peak | Full 1-thread substring scan |
| ------------------------------------ | --------- | --------- | ---------- | ---------------------------- |
| Original `BTreeMap<Arc<str>, Entry>` | 958 MiB   | 502 B     | n/a        | n/a                          |
| Compact tree catalog                 | 186 MiB   | 97 B      | 201 MiB    | about 100–150 ms             |
