# Actual-app extraction and embedding concurrency qualification

This is an unexecuted qualification plan for the bounded extraction candidate
`f42add8`, following the [resource policy](resource-policy.md). The decision is
to retain permitted-CPU inference selection for the first phase and record
that embedding throughput improvement is unproven. No physical-core or generic
half-core rule follows from the eight-physical/eight-logical host's results.

## Admission and comparison

First require the candidate's scheduler/app tests, strict Clippy and full app
compilation to pass. Obtain immutable baseline/candidate release apps and record
run ID, source commit, executable SHA and build provenance. Use a baseline with
the same resource policy and the `b876672` foreground-priority restoration.
Product-source differences must be limited to the extraction change; incidental
UI, priority, inference-thread or asset changes require a separate comparison.
The static merge of `f42add8` with `db8d43e` was conflict-free and retained that
priority restoration. It is not a compiled or executed app.

Run each pair A/B/B/A with profiling off. Use fresh owned portable profiles,
identical pinned EmbeddingGemma Q8 and b11960 CPU assets, one frozen synthetic
corpus, fixed settings and the same CPU/memory configuration. Assets are
imported only into the test profiles. No generation model or response stub is
needed. The corpus remains unchanged across cases; record its byte hash and
compare stored document/chunk identities and text, embedding dimensions and
cosine, indexing errors and completed counts.

| Case                         | Whole app and descendants                                              | Workload                                                                          | What it qualifies                                                         |
| ---------------------------- | ---------------------------------------------------------------------- | --------------------------------------------------------------------------------- | ------------------------------------------------------------------------- |
| Minimum baseline             | 1 CPU, 512 MiB                                                         | Existing filename startup/search, then non-OCR content extraction without a model | Existing low-end UI/filename target and serial extraction regression      |
| Single CPU with inference    | 1 CPU, explicitly declared sufficient RAM (initial proposal 6 GiB cap) | Real extraction, embedding indexing and foreground semantic queries together      | Shared-server queue/scheduling behavior on one CPU; not 512-MiB inference |
| Multiple CPUs with inference | All permitted CPUs, same declared RAM cap                              | Same real extraction, embedding and foreground queries                            | Whether extraction overlap helps or instead competes with inference       |

The 6-GiB cap is a proposed experimental bound, not a product minimum, memory
reservation or a promise of fit. Require 4 GiB available before a model run,
stop below 1 GiB, and apply the owned-Job 600-second bound. Read back actual
limits and membership for the model and every WebView child; do not exclude
them from peak memory. Record actual extraction workers, for example from owned
thread descriptions (`prism-content-extract`) rather than a predicted budget.
One-worker inline execution creates no extraction worker threads; it does not
exercise parallel extraction. Keep the same external sampler on all cases and
record its CPU cost and sampling lateness. The previous model
component peak exceeded 512 MiB, so its separate larger Job must never be
reported as a minimum-profile pass.

## Workload and event boundaries

Use a small fixed content corpus with enough parsing work to overlap inference,
while bounding indexed text/chunk count so the full case fits the deadline.
Keep source bytes, parser version, OCR disabled, output text and model prompts
identical. A formatting-heavy synthetic document with little visible text can
exercise parsing without generating thousands of embedding requests. Do not
fabricate extraction/embedding timing or replace any response.

Import and start the pinned model, index one fixed seed document and warm a
real foreground query before the measured bulk scope is introduced. This keeps
model/page-cache warmup out of the concurrent comparison and provides a real
searchable vector. Use the same seed and warmup on both apps, then observe the
actual `content_status` indexing/embedded counters. During a proven
interval of simultaneous preparation and real embedding, issue fixed semantic
searches through the actual app command/UI path. Repeat the same schedule after
indexing finishes as the quiet reference. Add filename-index contention using
the same controlled scope on both apps. A local whole-drive run can complement
a hosted synthetic run, but different live row counts/service cache states
cannot be treated as identical-input causal proof.

Record first search, full-ready and total embedding completion separately.
Measure foreground query enqueue/admission wait, HTTP inference service time,
vector-query completion and end-to-end UI result latency where observable.
Do not label the entire command wall time as inference time. Separately probe
an actual renderer heartbeat and the short index-status response. Keep the
query schedule fixed; do not launch uncontrolled concurrent searches that
cancel one another in the same window.

## Attribution and decision

Collect per-process and, where feasible, per-worker user/kernel CPU, cycles,
peak committed memory, input/output counts and errors. Read back process/thread
priority, boost flags and affinity: the app's below-normal indexing threads and
a below-normal runtime process need not have the same Windows base priority.
A runnable higher-priority background pool can delay the shared inference
server even if the renderer remains responsive. This is a hypothesis requiring
the concurrent measurement, not a measured regression.

Use only numeric event/latency samples and fixed synthetic IDs in exported
profiling evidence. Do not export user filenames, source paths, search text,
profiles, tokens or model bytes. Retain failed/aborted cases and check owned
process cleanup and original input hashes after every case.

Report total throughput together with foreground p50/p95/max and errors, not
only CPU utilization or one faster run. If the concurrent case loses time or
responsiveness, revise scheduling/concurrency before acceptance. The earlier
4/8 document-only comparison used more CPU without an established throughput
benefit; it cannot substitute for this experiment. No actual concurrent app
measurement has run yet, and the cached 5–7 second full-list target stays open.
