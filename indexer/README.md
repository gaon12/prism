# Optional Windows NTFS index service

This is the optional NTFS backend approved for Prism. Initial elevated
installation and SCM startup have been verified on the development machine.
The app prefers an already running service
for complete drive-root reconciliations and otherwise uses the parallel folder
walker. A 5–7 second C-drive index is still unverified.
The target is a cached start with about two million entries on one logical CPU,
512 MB RAM and an HDD. Cold MFT collection is measured separately. The complete
app/service memory budget and actual HDD timings remain unqualified.

Implemented:

- `protocol`: bounded, versioned binary frames for drive snapshots and changes.
  Requests accept a drive letter and revision, with no filesystem paths,
  program execution, file-content request, or write operation. Replies contain
  only filenames, directory flags, size and modification time.
  The client retains the codec's owned path-validation result for the stream
  receiver, avoiding a second scan. Constructed replies still require path
  checks; volume identity, ordering and global record/byte budgets are checked
  in both paths. The wire format remains version 2.
- `core`: read-only FILE-record decoding with sector fixups, base/extension
  references, hard links, DOS alias suppression, standard-information times,
  resident/nonresident data sizes, and parent-sequence checks. Junctions and
  symlinks are excluded; cloud placeholders can contribute names. Deleted,
  corrupt, orphaned, and reused references cannot invent live paths.
  Metadata is stored in ordered, fixed-size pages. Sequential MFT/cache records
  append without a large hash table or whole-table reallocation; out-of-order
  replay uses a small ordered overlay. Removed slots and large overlays are
  merged while consuming the old pages, so deletion history does not accumulate
  dead storage. Visitation and cache writing merge this order directly, without
  a separate whole-catalog sorting table. The cache format is unchanged.
  Derived paths reserve their complete UTF-8 byte size before appending the
  parent, separator and name; the separate Windows UTF-16 limit still applies.
- An independent public mkntfs image exercises 581 records, file sizes, and
  all 512 subdirectories. Adversarial unit tests cover bounds, torn sectors,
  cycles, cancellation, protocol lengths and path escapes.
- MFT bootstrap resolves fragmented data extents and resident/nonresident
  attribute lists. Bulk reads use bounded 4 MiB blocks and validate exact
  extension references, contiguous VCN coverage and nonoverlapping physical
  extents, rather than assuming the MFT is contiguous.
- Large MFT scans overlap those reads with at most four scoped record decoders.
  Each worker has one outstanding job; owned buffers of at most 4 MiB are moved
  and recycled, rather than copying the entire raw MFT again for a work queue.
  Source I/O, cancellation/deadline callbacks, progress, ordered catalog updates
  and journal replay stay on the calling thread. All workers are joined on
  success, cancellation and failure. Small scans retain the sequential reader.
  Filename, size, modification-time, hard-link and repair semantics are preserved.
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
  Initial markers are created with deletion-on-close and committed only after
  fsync, so forced process exit cannot strand a partial marker. Explicit service
  removal verifies all marker-reserved files before deleting any; legacy markers
  do not authorize deleting newer drive slots. A rejected cleanup preserves the
  SCM entry/image and restarts a previously running service. Unknown children
  prevent only nonrecursive directory removal and remain untouched.
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
verification is 76 tests on Windows and 38 portable tests under Linux;
all-target Clippy passes. These checks do not prove live C-drive speed.
Cache lifecycle regressions use only test-owned temporary files, including
forced termination of the test's own ordinary-user child. They do not claim
successful elevated removal of the actual installed service.

An explicit synthetic decoder/merge benchmark is available separately:

```sh
cargo test --release --locked --manifest-path indexer/Cargo.toml -p prism-ntfs-core --features cache --lib large_metadata_decode_and_merge_timing -- --ignored --nocapture --test-threads=1
```

It compares sequential and 2/4/8-worker runs over one million repeated resident
or mixed public-image records. Both paths simulate the same reads into reusable
buffers, and active metadata counts/checksums must agree. It prints three samples
and their median. This measures decoding/merging of synthetic memory input,
separately from installed-service I/O, cache/replay, IPC and app search readiness.
On the November 6 Windows run, four workers reduced median decode/merge time
from 576 to 516 ms for the resident input and from 832 to 502 ms for the mixed
public-image input. Eight workers did not improve either median. These are
synthetic stage timings, not a measured whole-drive speedup. The newer service
still needs deployment and live validation before making that claim.

An ordinary-user probe of the actual installed service is also available:

```sh
cargo run --release --manifest-path indexer/Cargo.toml -p prism-ntfs-windows --example service_probe -- --drive C
```

It reports aggregate snapshot counts/time without using an app profile or
clipboard. `--churn` additionally creates and cleans up only its own temporary
files to verify Unicode names, renames, deletions, descendant paths and sizes;
`--parallel` checks two concurrent clients. These options never install or
elevate a service.

For aggregate phase timing from an ordinary shell, use:

```sh
cargo run --release --manifest-path indexer/Cargo.toml -p prism-ntfs-windows --example service_timing -- --drive C
```

This prints counts/UTF-8 path-byte totals and three client-observed intervals:
request to first record, first-to-last record transfer, and the remaining wait
for terminal acknowledgement plus final root validation. It prints no paths and
never elevates, installs, starts or replaces the service. These intervals do not
separate the server's internal cache/MFT/replay work. Compare the first observed
request and an immediate repeat separately; neither is a controlled cold-start
measurement, and service transfer timing excludes the app's search-catalog work.

The app's user-requested diagnostic report also includes
`filenameIndex.lastServiceSnapshot` for the latest successfully validated NTFS
snapshot in the current configured generation. It contains only numeric timing,
count and initial-catalog information, with no paths, file names or drive identity.
Normal search/status replies are unchanged. Refresh/configuration clears this
record; a rejected or cancelled transfer cannot replace the previous success.

| Field               | Meaning                                                                                           |
| ------------------- | ------------------------------------------------------------------------------------------------- |
| `receivedRecords`   | Received records, including directory entries; this is not a count of unique physical files.      |
| `firstBatchMs`      | Collection entry to the first nonempty record batch, or null for an empty snapshot.               |
| `validatedFetchMs`  | Through completed native stream/root validation, overlapping private catalog preparation.         |
| `receiveCallbackMs` | Record callback wall time, including validation, queuing and backpressure; a subset of fetch.     |
| `finishPublishMs`   | Remaining worker joins, merging and publication after the validated fetch.                        |
| `totalMs`           | The snapshot collection/preparation/publication path; excludes watcher setup, final sweep and UI. |
| `initialCatalog`    | Whether the app catalog was empty when collection began.                                          |

These values cannot identify the server's internal MFT/cache/replay work, and
callback time must not be added to fetch time. The explicit ignored app test
`index::service::benchmark::live_streaming_installed_service_catalog` additionally
checks aggregate path/metadata preservation and prints these counters. Its
`qaMetadataHashingIncluded` flag notes the extra verification work inside the
measured fetch. This test requires an explicit ordinary-user C-drive opt-in and
never installs or manages the service. It does not prove whole-app GUI latency
or achievement of the 5–7 second target.

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

For an explicit CPU-only comparison of constructed and codec-validated stream
receivers, run:

```powershell
cargo run --release --manifest-path indexer/Cargo.toml -p prism-ntfs-protocol --example stream_validation -- --records 2000000
```

The example prepares identical synthetic wire frames outside the timer, reverses
variant order between rounds and consumes every decoded filename and metadata
field. It verifies counts and metadata checksums. It does not contact the service
or measure disk access, app-catalog publication or UI readiness.

Windows also has an explicit ignored `pipe::frame_bench::native_frame_write_comparison`
test in `prism-ntfs-windows`. It compares four-part, header/body and copied
whole-frame writes through real private fixture pipes with identical synthetic
bytes and decoded metadata. A 2-million-row run did not show a consistent timing
benefit from coalescing; the production framing remains unchanged. This test
never connects to an installed service or opens a volume.

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
   On November 6, the timing client observed about 2.04 million paths in
   19.057 seconds on its first request (14.951 seconds before the first record,
   3.079 seconds between first/last records, 1.026 seconds afterward). Its
   immediate repeat took 6.590 seconds (2.952 / 2.739 / 0.898 seconds).
   A separate current release filename harness reached app-catalog readiness in
   8.240 seconds without warnings. The drive contents, background load and cache
   state differ from the earlier measurements; this is a diagnostic baseline,
   not evidence of a controlled cold start, newer service deployment or a
   guaranteed 5–7 second app index.
   On November 8, an ordinary read-only client against the unchanged installed
   service observed about 2.34 million paths: 18.971 seconds on its first request
   and 7.597 seconds on its immediate repeat. The updated client took 7.806
   seconds in a later request. Duplicate stream-validation time fell from
   358 ms to 0.24 ms, but native read waits increased, so these runs do not
   demonstrate an overall wall-clock improvement. In a separate fixed-input
   2-million-row comparison, median decode/validation CPU time fell from 395 ms
   for the previous implementation to 213 ms for the updated implementation.
   These diagnostics exclude app-catalog publication and UI startup. The original
   whole-app C-drive 5–7 second target remains unmet; service wait time and actual
   deployment of the newer service still need investigation.
   A separate release-mode check of the current production filename modules
   received and published all 2,340,631 C-drive records in 8.696 seconds:
   8.081 seconds through authenticated transfer and overlapping private
   preparation, then 0.614 seconds to join, merge and publish. The stored paths,
   directory flags, sizes and timestamps matched the received metadata. This
   ordinary-client check used the unchanged installed service and excluded
   WebView/UI startup and subsequent query latency.
   Separately, a final-source synthetic catalog comparison emitted 2,001,756
   paths, preserving full metadata, hard links and MFT-number order. Across
   three paired visits, median path visitation plus metadata hashing fell from
   2.795 to 2.418 seconds after eliminating the second record lookup. The two
   catalogs used identical logical inputs and the same protocol dependency.
   This service-side code has not been deployed to the installed host, so the
   result does not establish a live C-drive or whole-app improvement.
   A later, separate comparison retained that lookup optimization and reserved
   the exact output string size for each derived path. With the same 2,001,756
   paths and complete metadata/order, its final-source three-pair median was
   2.443 seconds before and 2.234 seconds afterward. One pair was slightly
   slower; the earlier independent candidate comparison was faster in all
   three pairs (2.458 to 2.271 seconds at the median). These are synthetic
   visitation-and-hashing measurements, not cumulative savings to subtract
   from live app timings. Cache-read buffering and short-name validation
   experiments showed no consistent gain and were excluded; the snapshot codec
   and its digest/allocation limits remain unchanged.
   An ordinary native Windows portable app built from `93e3fa4` was also
   measured with a fresh profile and the unchanged installed service. From
   the filename-only C-drive settings request, after the GUI was already
   open, initial catalog usability took 20.194 seconds and the GUI count
   appeared at 20.278 seconds for 2,340,775 paths, including directories.
   The 7-second target was not met. A subsequent `cmd.exe` query returned
   83 matches, including `C:\Windows\System32\cmd.exe`, and rendered in
   451 ms. The indeterminate collection progress bar was visible, and the
   same search-to-chat row stayed mounted throughout the observed scan.
   No app JavaScript errors were recorded, and the 760-pixel layout fit.
   This was initial catalog readiness, not process-launch timing or a
   controlled cold-cache benchmark. Cache state, system load and individual
   transfer/preparation costs were not controlled or measured; subtracting
   older protocol-only timings would not identify a client-side bottleneck.
   No newer service was installed and no model or content indexing ran.
   A later ordinary-client release diagnostic, using filename source
   `7972ea2` with the same installed service, received and published all
   2,340,809 paths in 9.316 seconds. The first nonempty batch arrived at
   4.430 seconds; validated fetch with overlapping preparation took
   8.723 seconds, including 343 ms in record callbacks, followed by a
   592 ms join/merge/publication tail. Received and indexed counts and the
   full metadata aggregates matched. This diagnostic adds QA metadata hashing
   inside fetch and excludes GUI/watcher setup. Callback time is already part
   of fetch, and the first-batch delay does not isolate service CPU or wire
   time. Cache state and system load were uncontrolled; this is not a speed
   improvement over the earlier 8.696-second sample or a new GUI measurement.
   The existing service process and image remained unchanged, and the overall
   5–7 second target remains unmet.
   In a subsequent native Windows portable check of authenticated `2be96bc`,
   the initial catalog was usable at 8.519 seconds and the GUI showed the
   current count at 8.796 seconds. Its actual in-app snapshot diagnostic was
   8.379 seconds: first batch 3.380 seconds, validated fetch/preparation
   7.903 seconds and a 476 ms publication tail. The snapshot contained
   2,340,817 paths; at the GUI milestone the current catalog contained
   2,340,764, with seven skipped watch-update items. GUI verification bracketed
   the DOM read with the same current
   revision/count/phase, rather than requiring an old fixed count. The first
   fixed-count observer timed out and remains a failed observation. Real
   collection progress and the retained search-to-chat row were checked;
   no model/content indexing ran and the service image stayed unchanged.
   Cache/load remain uncontrolled, so this is neither a controlled improvement
   over the earlier GUI sample nor achievement of the 5–7 second target.
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
