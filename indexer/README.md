# Optional Windows NTFS index service

This is the start of the NTFS backend approved for Prism. **The service is not
installed, registered, or connected to the app yet.** The app currently uses
the parallel folder walker. A 5–7 second C-drive index is still unverified.

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

Run `cargo test --manifest-path indexer/Cargo.toml --locked` and
`cargo clippy --manifest-path indexer/Cargo.toml --locked --all-targets -- -D warnings`.
The current crates have no runtime dependency other than each other. `flate2`
is a test-only dependency for the compressed public fixture.

Remaining integration:

1. A Windows read-only volume adapter must resolve the complete MFT data
   runlist, including attribute-list extensions, and stream records in bounded
   blocks. It must not assume that the MFT is contiguous. MFT records can
   contain resident payload bytes; those are ignored and never returned by the
   metadata protocol, used as AI input, or retained in the catalog.
2. Capture a journal checkpoint before the snapshot, then replay changes before
   publishing it. Detect journal replacement/wrap and fail into a fresh snapshot.
   USN timestamps are journal-event times, not file modification times; preserve
   the standard-information timestamp and actual unnamed data size instead.
3. Implement the Windows service and local-only named pipe. Restrict its ACL,
   reject remote clients, verify the server against SCM, bound concurrent
   requests, apply I/O deadlines and propagate disconnect/cancellation.
4. Connect the ordinary-privilege app, active-scope filtering, status/progress,
   incremental updates and immediate scope revocation. Keep the parallel walker
   for unavailable services, non-NTFS disks, network folders and other OSes.
5. Add an optional, one-time UAC service installation into an administrator-owned
   location, uninstall/update handling and installer/portable UI. Installation
   must be explicit; ordinary app startup must never silently elevate itself.
6. Verify the live service and compare the same drive against the current
   walker. Fixture success is not a claim of Everything-level speed.

Implementation references: Microsoft's [file record segment header](https://learn.microsoft.com/en-us/windows/win32/devnotes/file-record-segment-header),
[attribute records](https://learn.microsoft.com/en-us/windows/win32/devnotes/attribute-record-header),
[volume data](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ni-winioctl-fsctl_get_ntfs_volume_data),
and [change journals](https://learn.microsoft.com/en-us/windows/win32/fileio/change-journals).
Everything's [service documentation](https://www.voidtools.com/support/everything/everything_service/)
describes how its NTFS service keeps the UI running without administrative rights.
