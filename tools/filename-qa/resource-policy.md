# Adaptive background resources

The 2026-11-16 decision treats one CPU/512 MiB as the minimum qualification
profile, while allowing additional cores and available RAM to accelerate work.
This first change provides a shared operation-start policy, rather than treating
four inference threads or eight filename workers as a universal machine size.

## Policy and bounds

[The production policy](../../src-tauri/src/resource_policy.rs) samples effective
CPU availability and memory at the start of work. Windows also intersects the
process affinity mask; processors outside that observed allowance are not added.
Cross-processor-group availability beyond what these APIs expose is not separately
qualified. There is no per-row/per-batch environment or memory probe.

Additional scratch concurrency uses `min(available / 4, 8 GiB)`, with a lower
budget of zero when the observation is absent/exhausted. A single existing
worker remains possible; the budget is not a new promise that its baseline
document/model allocation fits zero bytes. CPU work estimates 8 MiB per worker,
bounded by the effective CPUs, with at least one worker. Async readiness uses
2 MiB per scheduler thread and a separate 1-4 thread range. These estimates
size concurrency; they neither reserve RAM globally nor cap whole-process RSS.
Concurrent components and later memory-pressure changes still need admission
and workload-specific validation.

The current embedding implementation uses the same CPU choice, limited to 512
by the pinned runtime's
[GGML thread bound](https://github.com/ggml-org/llama.cpp/blob/b11960/ggml/include/ggml.h#L219).
The final embedding thread cap is awaiting the user's choice between the
permitted-CPU count and a maximum of four. Until that decision arrives, the
implementation remains unchanged. Its context, batch, model, authentication
and loopback policies are unchanged.
This does not enable a model, download assets, make external requests or assert
that model weights fit the 512-MiB filename qualification profile.

| Observed resources           | Scratch budget | CPU workers | Async scheduler |
| ---------------------------- | -------------: | ----------: | --------------: |
| 1 CPU, 512 MiB available     |        128 MiB |           1 |               1 |
| 32 CPUs, 16 GiB available    |          4 GiB |          32 |               4 |
| 1,024 CPUs, 64 GiB available |          8 GiB |       1,024 |               4 |
| Unknown available memory     |              0 |           1 |               1 |

The last high-core case is a pure policy test, not physical hardware evidence;
embedding would select at most 512 threads. No core is reserved idle when there
is sufficient budget. Priority gives foreground UI work precedence instead.

## Memory observations

- Windows starts with available physical RAM, then conservatively intersects
  process/Job commit limits minus their recorded peaks when the process is
  constrained. [Microsoft defines these independent limits and peaks](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_extended_limit_information).
  A peak is not current usage: this subtraction can keep concurrency low after
  a past high-water mark. Failed limit inspection falls back to one worker.
- Linux starts with `MemAvailable`, intersects the current cgroup and ancestors,
  and handles standard v2 and v1 memory-controller mounts. It uses
  [v2 current/max accounting](https://docs.kernel.org/admin-guide/cgroup-v2.html)
  and [v1 usage/limit accounting](https://docs.kernel.org/admin-guide/cgroup-v1/memory.html).
  V1 usage is approximate. Unsupported legacy mount layouts, malformed values
  and unreadable limits remain conservative. The ancestor walk has a 64-step
  bound and cannot escape its mounted controller.
- macOS estimates reclaimable headroom from free plus inactive pages using
  `host_statistics64`. Speculative pages are already part of free pages in
  [Apple's structure definition](https://github.com/apple-oss-distributions/xnu/blob/main/osfmk/mach/vm_statistics.h),
  so they are not added again. The acquired host port is released.

These observations can become stale as other applications allocate memory.
They do not weaken existing document/OCR size bounds, cancellation, permission
checks, cache validation or ownership lifetimes.

## Actual encoder thread comparison

A separate owned-runtime experiment used the pinned 309,855,456-byte official
EmbeddingGemma 2 Q8 model and b11960 Windows CPU archive, verified by their
existing product hashes. It used authenticated loopback requests, CPU-only
execution, eight allowed CPUs, a read-back 3-GiB runtime Job limit and
below-normal process priority. Five identical synthetic passages were measured
after a full-passage warmup in each 4/8/8/4 thread case. Startup was excluded.

| Case | Threads / batch threads | Five requests (ms) | Server CPU (ms) | Peak Job commit (MiB) |
| ---- | ----------------------- | -----------------: | --------------: | --------------------: |
| A1   | 4 / 4                   |         14,805.829 |      52,515.625 |              1,291.73 |
| B1   | 8 / 8                   |         13,895.173 |      58,656.250 |              1,293.52 |
| B2   | 8 / 8                   |         14,276.946 |      62,671.875 |              1,294.07 |
| A2   | 4 / 4                   |         12,857.318 |      46,484.375 |              1,291.50 |

Mean request wall was 13,831.574 versus 14,086.060 ms (+1.84% for eight),
whereas mean server CPU was 49,500 versus 60,664.063 ms (+22.55%). The A-case
spread exceeds the mean wall difference: this does not establish either a
throughput improvement or a stable slowdown. Prompt-token counts matched and
all 768-dimensional vectors had cosine 1.0 to their first-case references.
No inference responses were replaced. This tests the pinned runtime with
matching CLI settings, not the application adapter or GUI. Host-load
instrumentation preparation failed on a Windows text-decoding error before
launch; the original frozen controller ran unchanged, so no per-case host-load
counter is claimed.

The host reports eight cores and eight logical processors. A physical-core
heuristic would not explain a preference for four here, so none was added and
there is no special case for this machine. The new policy exposes the allowed
capacity requested by the user; this corpus provides **no measured inference
speedup** for that wider capacity and records the extra CPU cost explicitly.
Workload-dependent model tuning and more hardware measurements remain open.

All four cases passed in 70.578 seconds, minimum available RAM was
4,217,229,312 bytes, all owned processes exited and source/model hashes stayed
unchanged. The read-only source model stayed in place; a verified private copy
and newly downloaded pinned runtime were used. Evidence is
`embedding-thread-qualification.json`, `embedding-thread-proof-inputs/` and
`embedding-thread-qualification/abba-v1/` under the existing private parent.

## Production consumers and foreground priority

- Filename preparation/walking/search use the shared budget in place of the
  eight-worker ceiling. Single-CPU searches skip the unnecessary memory probe.
- The embedding server receives adaptive `--threads` and `--threads-batch`.
- The dedicated content Tokio scheduler selects 1-4 I/O threads. Its shared
  vector-query workers retain normal priority after `b876672`; background
  scans lower their own monitor thread.
- Filename workers use the per-thread helper. Windows uses below-normal priority
  with I/O completion boosts disabled; Linux lowers only the calling task's
  nice priority; macOS uses utility QoS.
- The owned macOS OCR worker also renders collection reader PDF/Office pages.
  Its explicit priority downgrade was removed in `b876672`. It is not an
  index-only worker, and foreground response timing still needs qualification.
- Only newly owned runtime processes are deprioritized. Windows changes the
  owned Child handle, with boosts disabled. Unix sets nice priority in the
  private supervisor before exec, which its actual runtime child inherits.
  The app/UI process, installed service and unrelated processes are untouched.

Fixed two-thread runtimes inside the generation/RAG/document/vector test modules
are fixtures, not product inference limits, and remain unchanged. This change
does not yet parallelize content extraction, change native OCR admission from
two leases, or update Paddle's pinned four-thread Python worker. Those are
separate follow-ups with memory, cancellation and source-policy regressions.

## Local verification scope

The phase-one source `a0bca95` passed release filename QA with 148 tests
(8 ignored) and profiling off and on, plus strict all-target release Clippy and
the release probe build. Later fork-safety and shared-worker priority changes
need their own hosted checks; these historical passes do not execute them.
Regression coverage includes affinity, unknown/exhausted memory, monotonic
budgets, the 8-GiB cap, 32-core selection, the inference bound, nested quota
headroom, namespace boundaries and an owned child's priority/boost flags.
The child's fixture is ignored in ordinary enumeration and executed explicitly
by its owning parent test. Platform-module compilation and actual release-app
qualification remain separate from these filename checks.

The production embedding/process/OCR consumers additionally passed strict
release library Clippy in `platform-qa` in 316.875 seconds (minimum available
RAM 3,707,809,792 bytes). That compiles the real adapters, but is not a full
platform test-suite execution or a Tauri app build. macOS/Linux compilation
and native priority behavior remain for hosted CI/native qualification.

The opt-in `resource_budget` example waits for `G` before sampling. A numeric
controller applied and read back real 512-MiB process/Job limits, first with one
CPU and then all eight permitted CPUs on this host. The final probe selected
CPU/inference/I/O counts **1/1/1** and **8/8/4**, with scratch observations
133,744,640/133,721,088 bytes. It finished in 0.172 seconds, minimum available
RAM 4,792,856,576 bytes, no remaining owned process and unchanged source/PE
hashes. This verifies parameter selection and constraints, not inference
throughput, physical high-core hardware, whole-app latency or startup gain.

An initial diagnostic controller failed its final Job PID-list assertion after
both child probes exited successfully; that attempt is unqualified and retained.
The repaired controller records exit codes, closes the Job unconditionally and
checks all owned handles after closure. An earlier all-target compile also
caught a QA CLI dependency on `usable_cpus`; the final source retains that
helper and its tests. Failed attempts were preserved instead of relabelled.

Evidence remains outside every checkout under
`C:/Users/solso/prism-agent-work-20261115/`: `adaptive-resources-*-v5/`,
`adaptive-resource-live-probe-v{1,2,3}/` and their `run_*` controllers.
No live service, user profile, model download or private capture was used in
these checks. New-app one-CPU/512-MiB GUI regression, actual embedding throughput
and full cross-platform CI are still required; the cached 5-7 second full-list
target remains open.

## Follow-up: shared interactive embedding and Unix fork safety

The same owned embedding server serves both background document inputs and
interactive `embed_query` calls. Both use one inference mutex. A document batch
releases that mutex after each individual input; there is no priority queue or
foreground exception. The previous all-eight-CPU document-only 4/8 comparison
therefore cannot establish interactive latency on one CPU.

A follow-up moves the Unix priority query before `fork`. The child hook captures
`max(inherited_nice, 5)` and only calls `setpriority`, then returns `Ok(())` even
if the policy call fails. The parent-side query distinguishes a valid nice value
of -1 from an error using thread-local errno; an unreadable priority leaves
inherited scheduling untouched. It never tries to raise an already lower-priority
child. A bounded owned-child Unix regression checks the post-exec priority and
unchanged parent; it is prepared for hosted macOS/Linux execution, not locally
verified on Windows. No query-priority exception is implemented by this change.

Qualification must compare normal/below-normal priority with fixed one-thread
inference, pinned assets, fixed synthetic query/document inputs and controlled
background contention. Separate admission/queue wait from HTTP inference time,
compare vectors/token counts and record host load, priority/boost/affinity
readback, peak memory and UI responsiveness. Include both quiet and contended
cases and paired order; do not attribute thread-count or model-load changes to
priority. Then verify the actual app path.

The previous runtime component test peaked at 1,356,926,976 bytes (about
1.26 GiB), so it does not prove
that optional inference fits the whole-app 512-MiB Job. Keep the one-CPU/512-MiB
filename/UI baseline separate from a one-CPU inference component run with its
actual declared memory limit. Current local RAM is below the 4-GiB model-test
preflight; no interactive-priority timing has been measured. If it regresses,
review a foreground-safe shared-server design before accepting the policy.
[Linux priority semantics](https://man7.org/linux/man-pages/man2/getpriority.2.html)
require sufficient privilege or an appropriate `RLIMIT_NICE` to undo a nice
increase. Simply boosting the same server around each query is therefore not a
portable assumed fix.

The existing real-model 4/8/8/4 comparison processed the same five inputs per
case: four-thread mean 13.832 seconds versus eight-thread mean 14.086 seconds,
with CPU time 49.500 versus 60.664 seconds. The wall difference is inside the
four-thread run-to-run spread; no eight-thread throughput benefit is established.
This host reports eight physical and eight logical cores, so four threads is
an `n/2` comparison rather than its physical-core count. That result does not
justify a universal half-core rule. The user decision between permitted-CPU
selection and a maximum of four embedding threads is still pending. Current
main uses permitted-CPU selection; that describes the implementation, not an
accepted final policy. No policy change is made while awaiting the answer.
Embedding throughput improvement remains unproven. Actual concurrent
extraction/embedding and foreground response measurements will inform later
adjustments.
The thread-count selection probe in a 512-MiB Job executed no model and cannot
establish model/context memory fit.

The next acceptance step is the [actual-app concurrent extraction and embedding
comparison](content-concurrency-qualification.md), including foreground queries
and filename contention. This plan is unexecuted.
