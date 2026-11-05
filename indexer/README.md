# Optional Windows NTFS index service

This is the optional NTFS backend approved for Prism. **The service is not
installed or registered yet.** The app now prefers an already running service
for complete drive-root reconciliations and otherwise uses the parallel folder
walker. A 5–7 second C-drive index is still unverified.

Implemented:

- `protocol`: bounded, versioned binary frames for drive snapshots and changes.
  Requests accept a drive letter and revision, with no filesystem paths,
  program execution, file-content request, or write operation. Replies contain
  only filenames, directory flags, size and modification time.
- `core`: read-only FILE-record decoding with sector fixups, base/extension
  references, hard links, DOS alias suppression, standard-information times,
  resident/nonresident data sizes, and parent-sequence checks. Junctions and
  symlinks are excluded; cloud placeholders can contribute names. Deleted,
  corrupt, orphaned, and reused references cannot invent live paths.
- An independent public mkntfs image exercises 581 records, file sizes, and
  all 512 subdirectories. Adversarial unit tests cover bounds, torn sectors,
  cycles, cancellation, protocol lengths and path escapes.
- MFT bootstrap resolves fragmented data extents and resident/nonresident
  attribute lists. Bulk reads use bounded 4 MiB blocks and validate exact
  extension references, contiguous VCN coverage and nonoverlapping physical
  extents, rather than assuming the MFT is contiguous.
- `windows`: read-only volume handles, NTFS 3.x geometry, page-aligned buffers,
  overlapped sector-aligned reads and owned cancellation cleanup. Actual native
  I/O tests use a test-owned regular file; live privileged volume reads remain
  unverified.
- A private snapshot builder replays the USN interval after a bulk scan, then
  re-reads complete metadata families. Deleted or reused references, directory
  renames and torn records are handled before publication. Deterministic
  driver/journal fixtures test this ordering on the independent NTFS image.
- An SCM-hosted Windows service and local-only named pipe support bounded
  snapshot requests, cancellation, deadlines and completion acknowledgements.
  The ordinary-user client verifies the pipe server against the running SCM
  process and binds replies to both the serial and kernel device of the open
  NTFS root handle. Protocol v2 rejects a different cloned volume even when its
  serial matches. No install or elevation occurs when running the binary.

Run `cargo test --manifest-path indexer/Cargo.toml --locked` and
`cargo clippy --manifest-path indexer/Cargo.toml --locked --all-targets -- -D warnings`.
The core/protocol crates have no external runtime dependencies. The Windows
adapter uses `windows-sys`; `flate2` and `tempfile` are test-only dependencies.

An explicit read-only probe is available for an already authorized Windows
administrator shell:

```sh
cargo run --release --manifest-path indexer/Cargo.toml -p prism-ntfs-windows --example snapshot_probe -- --drive C
```

It prints aggregate counts and timing, never paths or file contents, and does
not request elevation or install a service. In an ordinary-privilege session,
`PermissionDenied` and exit code 2 are expected. Current verification is 41
tests on Windows and 29 portable tests under Linux; all-target Clippy passes
on both. These checks do not prove live C-drive speed.

Remaining integration:

1. Verify the implemented adapter against a live, explicitly authorized NTFS
   volume, including journal replay and recovery under real churn. MFT records
   can contain resident payload bytes; those are ignored and never returned by
   the metadata protocol, used as AI input, or retained in the catalog.
2. Verify the service through an actual SCM installation. Current native pipe
   tests use a private test endpoint; they do not impersonate an installed
   service. Delta requests currently return `JournalGap`, explicitly requiring
   a new snapshot rather than claiming an unchanged catalog.
3. Finish service progress details and USN-based incremental updates. The app
   already validates the complete snapshot before publication, cancels idle
   requests on scope revocation, filters to the active root, and retains the
   parallel walker for unavailable services, subfolders and other OSes.
4. Add an optional, one-time UAC service installation into an administrator-owned
   location, uninstall/update handling and installer/portable UI. Installation
   must be explicit; ordinary app startup must never silently elevate itself.
5. Verify the live service and compare the same drive against the current
   walker. Fixture success is not a claim of Everything-level speed.

Implementation references: Microsoft's [file record segment header](https://learn.microsoft.com/en-us/windows/win32/devnotes/file-record-segment-header),
[attribute records](https://learn.microsoft.com/en-us/windows/win32/devnotes/attribute-record-header),
[volume data](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ni-winioctl-fsctl_get_ntfs_volume_data),
and [change journals](https://learn.microsoft.com/en-us/windows/win32/fileio/change-journals).
Everything's [service documentation](https://www.voidtools.com/support/everything/everything_service/)
describes how its NTFS service keeps the UI running without administrative rights.
