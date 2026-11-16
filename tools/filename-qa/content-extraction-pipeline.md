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

## Source authority

[Content preparation](../../src-tauri/src/content_service.rs) rechecks the request
and path admission when a queued file reaches a worker, before the fingerprint
or extraction reads. The final fresh admission and SQL publication retain the
configuration lock. Repository-rule changes can therefore reject already
prepared text. Source fingerprint validation, extracted-text reuse, disabled
image handling, embedding order and the external status/command DTOs remain.

## Required verification

The source includes five scheduler regressions: inline execution, the bounded
reorder window, consumer stop, cancellation/join and panic handling. App tests
compare real sequential/parallel preparation and SQL summaries, reject prepared
text after cancellation/generation changes and recheck changed repository rules.
Existing category-change/battery/cancellation regressions must also pass.

The optional component benchmark uses the exact extractor, scheduler and SQL
store on 96 fixed synthetic HTML files. All four 1/N/N/1 cases must publish the
same semantic SHA, with unchanged input SHA. It includes serialization/hash and
SQL costs; it does not exercise app admission, embedding or GUI latency.

```powershell
$env:PRISM_DEV_CONTENT_PIPELINE_QUALIFY = '1'
cargo test --manifest-path tools/platform-qa/Cargo.toml --locked --offline -j1 --release --lib content_pipeline_qualification::actual_extraction_window_qualification -- --ignored --exact --nocapture --test-threads=1
```

It is ignored in ordinary tests and returns before file creation unless its
explicit environment flag is set. Temporary files contain only generated text.
Use the owned-process/memory guard for every execution; do not run against a
user's content roots or profile.

At candidate preparation, the local guard observed only 824,610,816 bytes of
available RAM (required 3,221,225,472) and refused to start Cargo. Formatting and
static review are separate from compilation: no new tests, Clippy, benchmark,
full-app build or one-CPU/512-MiB GUI regression had run at that point. Those
checks and measured throughput evidence are required before acceptance. No
speedup or cached 5–7 second target acceptance is claimed.
