# Bounded content extraction candidate

This follow-up to the [resource policy](resource-policy.md) prepares supported
content documents concurrently when OCR is disabled and the request-start CPU
and RAM budget admits more than one worker. It keeps one worker inline, without
channels or new extraction threads. OCR-enabled requests remain serial pending
separate native/Paddle concurrency qualification.

## Budget and scheduling

A slot estimates 512 MiB for parser input/expansion, text, result storage and
working allocations. The [shared policy](../../src-tauri/src/resource_policy.rs)
allows at most a quarter of available memory, capped at 8 GiB, and intersects
workers with permitted CPUs. Unknown/insufficient headroom retains one worker.
Thus extraction selects 1–16 workers. This estimate controls concurrency; it is
not a reservation or an RSS guarantee. Existing per-document limits (64 MiB
source, 128 MiB expanded input, 32 MiB text and bounded sections/archive entries)
remain unchanged. Memory shared with embedding or another operation is not
reserved globally.

The [window](../../src-tauri/src/content_pipeline.rs) counts all issued but not
published work: queued tasks, active parsers, channel results, buffered results
and the consumer's current result. That total never exceeds the number of
successfully spawned workers. A stalled first document cannot make later
results accumulate indefinitely. Failed thread creation reduces the window;
zero successful spawns falls back to inline work. Workers use the same
background-priority helper as the resource policy.

The scanning thread alone stores documents and performs embedding, in discovery
order. Workers do not take the configuration/publication lock. Cancellation or
a generation change stops dispatch/publication, drops pending results, closes
channels and joins all started workers. Parsing retains its existing cooperative
cancellation checks; joining does not forcibly terminate a non-cooperating parser.
A worker panic reports a failed scan rather than waiting for a missing result.

## Storage admission

Parallel reads require explicit non-seek evidence for every selected root. An
HDD, unsupported filesystem/device, failed query or unknown device keeps the
whole request at one worker. A CPU/RAM/OCR decision already at one worker skips
all new storage queries, retaining the minimum-profile inline path.

The request-local [storage policy](../../src-tauri/src/content_storage.rs) caches
at most 128 device decisions. Windows resolves each path's actual volume mount
and volume GUID, opens an owned volume handle with no data access, then queries
[StorageDeviceSeekPenaltyProperty](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ns-winioctl-device_seek_penalty_descriptor).
Only a complete valid descriptor with no seek penalty admits parallelism.
Denied/unsupported queries are serial; there is no elevation. Raw output bytes
avoid constructing an invalid Rust boolean from malformed descriptor data.

Linux resolves the path's `st_dev` via `/sys/dev/block`, walks partitions to
their block queue and reads the kernel's
[rotational indicator](https://github.com/torvalds/linux/blob/master/Documentation/ABI/stable/sysfs-block).
All stacked-device slaves must also be known nonrotational. Missing/invalid
values, cycles, excessive graph sizes and virtual leaves with unknown backing
storage are serial. macOS has no seek-penalty probe here; current Macs boot
from internal SSDs, so each existing mounted device (keyed by its device id) is
treated as non-seek and may use parallel workers. External Mac HDD detection is
a follow-up, and native Mac parallel throughput is not yet measured.

An SSD root can contain a mounted HDD or network filesystem. The producer
therefore resolves each candidate file's actual device. A newly encountered
seek-sensitive/unknown device drains and publishes all preceding work, then
extracts that file inline with no other issued extraction. Later eligible SSD
files may resume parallel preparation. Publication order and cancellation checks
remain unchanged. Device observations control scheduling only; they do not
replace the fresh source authority/fingerprint checks or freeze mount changes.

The OS API path was queried read-only on this machine's two existing volumes:
both returned a complete 12-byte non-seek descriptor and all owned handles were
closed. That probe used no source contents or elevation. It did not execute the
Rust adapter, test an actual HDD, or qualify throughput/GUI behavior. This host
has no supplied HDD; real HDD performance remains unverified.

## Source authority

[Content preparation](../../src-tauri/src/content_service.rs) rechecks the request
and path admission when a queued file reaches a worker, before the fingerprint
or extraction reads. The final fresh admission and SQL publication retain the
configuration lock. Repository-rule changes can therefore reject already
prepared text. Source fingerprint validation, extracted-text reuse, disabled
image handling, embedding order and the external status/command DTOs remain.

## Required verification

The source includes six scheduler regressions: inline execution, the bounded
reorder window, consumer stop, cancellation/join, panic handling and draining
before serial tasks. Storage regressions cover per-device caching, cache limits,
malformed/truncated descriptors, rotational values and stacked/cyclic devices. App tests
compare real sequential/parallel preparation and SQL summaries, reject prepared
text after cancellation/generation changes and recheck changed repository rules.
Existing category-change/battery/cancellation regressions must also pass.

The optional component benchmark uses the exact extractor, scheduler and SQL
store on 96 fixed synthetic HTML files. All four 1/N/N/1 cases must publish the
same semantic SHA, with unchanged input SHA. It includes serialization/hash and
SQL costs and requires a verified non-seek source volume before running the
parallel cases. It does not exercise the full app admission path, embedding or
GUI latency. An unknown device, or a Mac volume admitted only by the SSD
assumption, cannot qualify this parallel component case.

```powershell
$env:PRISM_DEV_CONTENT_PIPELINE_QUALIFY = '1'
cargo test --manifest-path tools/platform-qa/Cargo.toml --locked --offline -j1 --release --lib content_pipeline_qualification::actual_extraction_window_qualification -- --ignored --exact --nocapture --test-threads=1
```

It is ignored in ordinary tests and returns before file creation unless its
explicit environment flag is set. Temporary files contain only generated text.
Use the owned-process/memory guard for every execution; do not run against a
user's content roots or profile.

At initial candidate preparation, the local guard observed only 824,610,816
bytes of available RAM; the storage follow-up recheck observed 1,271,660,544.
Both were below the required 3,221,225,472 bytes and refused to start Cargo.
All three workspace metadata checks passed with `--locked --offline` and
unchanged lock files; the two needed windows-sys features reuse the existing
crate/version. Formatting and static review are separate from compilation: no new tests, Clippy, benchmark,
full-app build or one-CPU/512-MiB GUI regression had run at that point. Those
checks and measured throughput evidence are required before acceptance. No
speedup or cached 5–7 second target acceptance is claimed.
