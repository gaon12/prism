# Optional Windows NTFS index service

This is the optional NTFS backend approved for Prism. Initial elevated
installation and SCM startup have been verified on the development machine.
The app prefers an already running service
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
  I/O tests use a test-owned regular file. Explicit live read-only C-volume
  probes also exercise metadata collection and journal replay.
- A private snapshot builder replays the USN interval after a bulk scan, then
  re-reads complete metadata families. Deleted or reused references, directory
  renames and torn records are handled before publication. Deterministic
  driver/journal fixtures test this ordering on the independent NTFS image.
  Raw disk records require valid sector markers and restoration. Completed
  filesystem-driver record buffers are already restored; cached metadata edits
  can leave their old update-sequence arrays stale, so these buffers are
  structurally validated without applying that array a second time.
- Core incremental replay consumes an existing snapshot and refreshes only
  changed metadata families. Old extension ownership survives merging, so
  deleted/reused extensions also refresh their surviving old bases. Inconsistent
  family reads abort instead of inventing deletion of a still-present base.
  The optional `cache` codec persists names/metadata/checkpoints with a bounded,
  versioned SHA-256 payload and physical-device/geometry checks. Digest
  verification precedes catalog allocation. The Windows backend now loads this
  cache and replays the journal before serving a snapshot; incompatible caches
  and lost journals require a fresh scan.
- Persistent metadata uses a fixed private service subdirectory, exact protected
  ownership/ACL checks and regular single-link files. Immutable read handles
  prevent changes during hash verification and decoding. Writes use a bounded
  staging file, fsync and same-directory replacement. Per-drive slots have a
  combined 2 GiB committed budget; one shared staging file can temporarily add
  at most the codec's 1 GiB payload bound plus its header. Only reserved, verified
  cache files can be evicted. Unknown children are preserved.
  A single full-catalog owner serializes snapshots and deferred writes; idle
  service memory does not retain a catalog for every drive. Deferred persistence
  follows record transfer and has its own shutdown/deadline checks. Cache failure
  does not relax filesystem trust or invalidate an otherwise verified snapshot.
- An SCM-hosted Windows service and local-only named pipe support bounded
  snapshot requests, cancellation, deadlines and completion acknowledgements.
  The ordinary-user client verifies the pipe server against the running SCM
  process and binds replies to both the serial and kernel device of the open
  NTFS root handle. Protocol v2 rejects a different cloned volume even when its
  serial matches. Connecting or invoking `--service` never installs or elevates.
- A separate `prism-file-index-admin` helper embeds the compiled host payload.
  Its explicit `--install`/`--start`/`--upgrade`/`--uninstall` entry points require an
  already elevated token, use a fixed Program Files
  directory with administrator ownership and protected ACLs, and reject
  writable ancestors or reparse points. Initial installation never replaces
  existing services/binaries. Explicit upgrade verifies the exact existing
  service, stages only the helper's embedded image and retains a hard link to
  the original until the new service starts. A failed start restores the old
  image; interrupted artifacts block another upgrade and remain for recovery.
  Removal checks the exact service executable before stopping it.
  On standalone machines, an unresolved grant is ignored only after local LSA
  namespace validation and an exact missing-account result. Owners remain
  restricted to trusted system principals. Joined/unknown machine roles keep
  the stricter policy because name lookup can fail during network outages.
  Starting/removing verifies the exact own-process LocalSystem service before
  acting. The host itself accepts only `--service`; it cannot install itself.
  Initial elevated deployment and guarded replacement are verified. Full
  crash-recovery and installer lifecycle integration remain pending.
  An explicit live fault-injection check used a valid PE image with the wrong
  service role: SCM reported 1053, the helper restored the original hash and
  running state, and no staging/backup files remained. This failure test is
  separate from the real host used for successful snapshot measurements.

Run `cargo test --manifest-path indexer/Cargo.toml --locked` and
`cargo clippy --manifest-path indexer/Cargo.toml --locked --all-targets -- -D warnings`.
Core/protocol metadata parsing has no external runtime dependencies. The optional
cache codec uses `sha2`; enable it with `--features prism-ntfs-core/cache` in
workspace checks. The Windows adapter uses `windows-sys`; `flate2` and
`tempfile` are test-only dependencies.

An explicit read-only probe is available for an already authorized Windows
administrator shell:

```sh
cargo run --release --manifest-path indexer/Cargo.toml -p prism-ntfs-windows --example snapshot_probe -- --drive C
```

It prints aggregate counts and timing, never paths or file contents, and does
not request elevation or install a service. In an ordinary-privilege session,
`PermissionDenied` and exit code 2 are expected. With the administrative helper
and cache codec enabled (`--features admin,prism-ntfs-core/cache`),
verification is 69 tests on Windows and 38 portable tests under Linux;
all-target Clippy passes. These checks do not prove live C-drive speed.

An ordinary-user probe of the actual installed service is also available:

```sh
cargo run --release --manifest-path indexer/Cargo.toml -p prism-ntfs-windows --example service_probe -- --drive C
```

It reports aggregate snapshot counts/time without using an app profile or
clipboard. `--churn` additionally creates and cleans up only its own temporary
files to verify Unicode names, renames, deletions, descendant paths and sizes;
`--parallel` checks two concurrent clients. These options never install or
elevate a service.

The app's Windows build compiles and embeds both images automatically. For
standalone helper validation, build the real host first (PowerShell):

```powershell
cargo build --manifest-path indexer/Cargo.toml --target-dir indexer/target --locked --release -p prism-file-index --bin prism-file-index
$env:PRISM_INDEX_SERVICE_PAYLOAD = (Resolve-Path indexer/target/release/prism-file-index.exe).Path
cargo test --manifest-path indexer/Cargo.toml --target-dir indexer/target --locked --all-targets --features admin
cargo build --manifest-path indexer/Cargo.toml --target-dir indexer/target --locked --release -p prism-file-index --bin prism-file-index-admin --features admin
```

The helper embeds these bytes during compilation; neither callers nor an
environment variable can choose a payload or destination at runtime.

Remaining integration:

1. Continue live churn/recovery coverage. The C-volume collector completed four
   consecutive read-only probes after correcting cached-record decoding; this
   does not measure IPC or app search readiness. MFT records
   can contain resident payload bytes; those are ignored and never returned by
   the metadata protocol, used as AI input, or retained in the catalog.
2. Expand live installed-service coverage. With the first synchronous disk-cache
   implementation, ordinary debug snapshots of about 2.19 million C-drive paths
   took 18.0 seconds cold and 10.8 seconds warm. Process read I/O fell from
   6.75 GB to 341 MB; these are process counters, not physical-disk measurements.
   A release probe verified actual owned-file churn. Against that same installed
   host, the updated production filename client reached search readiness in
   9.9 seconds versus 10.6 seconds before its allocation changes, with sampled
   peak private memory reduced from 1.99 GB to 1.28 GB. These separate runs are
   not a 5–7 second cold claim, nor proof of the newer deferred/per-drive cache.
   Separately, the full unoptimized debug app collected 2.20 million entries
   without warnings in 25.8 seconds including WebView startup; that measurement
   is not interchangeable with the release filename harness.
   Delta requests currently return `JournalGap`, requiring
   a new snapshot rather than claiming an unchanged catalog.
3. Verify the newer deferred/per-drive persistence through actual deployment,
   restart and concurrent-client checks, and finish service progress details. The app
   already validates the complete snapshot before publication, cancels idle
   requests on scope revocation, filters to the active root, and retains the
   parallel walker for unavailable services, subfolders and other OSes.
4. Verify elevated deployment and complete rollback/recovery, version update
   handling, cache cleanup and installer removal integration. The Windows app now embeds the
   matching helper and offers explicit UAC actions under search-scope settings.
   Read-only preflight precedes UAC; isolated profiles reject all changes before
   extraction. Native UI tests use real SCM/IPC and never fake an installation.
   Permission refusals must not modify system directory ACLs.
5. Verify the live service and compare the same drive against the current
   walker. Fixture success is not a claim of Everything-level speed.

Implementation references: Microsoft's [file record segment header](https://learn.microsoft.com/en-us/windows/win32/devnotes/file-record-segment-header),
[attribute records](https://learn.microsoft.com/en-us/windows/win32/devnotes/attribute-record-header),
[volume data](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ni-winioctl-fsctl_get_ntfs_volume_data),
and [change journals](https://learn.microsoft.com/en-us/windows/win32/fileio/change-journals).
Everything's [service documentation](https://www.voidtools.com/support/everything/everything_service/)
describes how its NTFS service keeps the UI running without administrative rights.
Service management follows the Windows [service access model](https://learn.microsoft.com/en-us/windows/win32/services/service-security-and-access-rights)
and [CreateService](https://learn.microsoft.com/en-us/windows/win32/api/winsvc/nf-winsvc-createservicew).
Account classification uses [local LSA policy](https://learn.microsoft.com/en-us/windows/win32/api/ntsecapi/nf-ntsecapi-lsaqueryinformationpolicy),
[machine roles](https://learn.microsoft.com/en-us/windows/win32/api/dsrole/nf-dsrole-dsrolegetprimarydomaininformation)
and the documented [SID lookup failure limits](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-lookupaccountsidw).
