# Actual-generation fixtures

The three Markdown files are public, test-authored source documents. The native
generation gate checks actual pinned models against their dates, amounts and
locations; it does not substitute generated answers.

The pinned Qwen models are test fixtures, not Prism's default generative model.
New profiles have no local or external generation configuration. Users configure
an OpenAI-compatible server and model; Prism does not install or start that
generative server. The native tests launch their own owned server and explicitly
configure their private app profile. Changing a qualification fixture therefore
does not change user settings or introduce a managed product model.

Qwen3-1.7B Q8 remains the fixture for the required CI generation gate. The 4B
qualification stays opt-in; its success does not satisfy or replace that required
gate. This policy applies to test fixtures. Product profiles continue to require
the user's server and model configuration.

[Windows CI run 43446497263](https://github.com/gaon12/prism/actions/runs/43446497263)
passed the original actual 1.7B generation gate at `0d15e44`: the real editor saved
the reviewed five-field layout, the generated HWPX contained the requested table
and absence statement, and reopening the generated document and answering from it
passed. The result records actual inference, no response replacement, the pinned
assets, a verified private portable profile, no console errors/warnings and normal
app exit. Its captured scripts and three source fixtures match that Git commit;
the artifact digest and HWPX were independently checked. The same Windows job
also passed the separate document PDF/HWPX and download-cancellation checks.

This qualifies that earlier label-based implementation, not the newer
`4371494` value/quote-only extraction contract. The latter's component results and
subsequent native qualification at `ead2af3` are recorded below.
One successful native fixture does not establish model stability or arbitrary
document correctness. Evidence:
`.dev/ci-43446497263-native-evidence-20261114/qualification.json`, SHA-256
`1c8d712fd1470509d332f4c7dbec683a2444436209137fa0e3c0994fdf3c58db`.

The required [Windows CI run 43377675738](https://github.com/gaon12/prism/actions/runs/43377675738)
at `c8bec76` remains red for actual Qwen3-1.7B generation. Its body preserved the
workshop date, location, budget and attendance in four fact paragraphs, but
produced no table and omitted the requested reservation-absence note. The six
supplied excerpts included paragraph 9, which explicitly records that absence;
this failure therefore persists after retrieving the relevant evidence. The
actual HWPX contained zero tables. The original assertions and failure remain
preserved; this is separate from the synthetic document/export smoke test.

A separate direct-model control on 2026-11-13 moved only the existing
product-owned document rules from the final user message to the system message.
It used the pinned 1.7B Q8 model and b11960 CPU runtime, temperature 0, reasoning
off, 8192 context and 2048 output tokens. Each request used a fresh server with
zero cached prompt tokens. The reviewed input, six excerpts, prior turns and
JSON schema were otherwise identical; reversing the placement reproduced the
baseline request exactly.

| Rule placement     | HTTP seconds | Prompt / completion tokens | Blocks | Tables | Reservation-absence note |
| ------------------ | -----------: | -------------------------: | -----: | -----: | ------------------------ |
| Final user message |       41.609 |                 1683 / 372 |      4 |      0 | Absent                   |
| System message     |       37.562 |                 1683 / 249 |      2 |      0 | Absent                   |

Both responses matched the requested JSON schema, covered the reviewed sections
and used known source labels, but failed the unchanged facts/table assertions.
Both stopped normally without truncation. The pair completed in 86.28 seconds,
with minimum available memory 1.64 GiB, no resource/deadline stop and unchanged
inputs, model, archive and runtime files. The earlier controller attempt that
failed its 4 GiB preflight started no model and is retained separately; it is
not a model-quality result.

This is a controlled earlier request/history with current product rule
literals, not an exact current CI wire capture or a native-app qualification.
It shows that relocating these rules alone does not fix this frozen input. The
failed responses are preserved, and no production role change is adopted.

### Decision if rule placement does not resolve the failure

The direct-model control changes only the placement of existing
product-owned document rules from the final user message into the system
message. Source excerpts, user-reviewed input, previous turns, JSON schema,
model/runtime pins and decoding settings remain unchanged. A successful single
pair would support that hypothesis for the frozen input, not establish stable
actual-app generation. The original native gate must still pass after a real
implementation; the captured requests reconstruct a controlled earlier history
and are not an exact wire capture of the current CI run.

The current body schema constrains block fields and section IDs. Backend
validation also requires every reviewed section to be represented. Neither
contract makes free-form Markdown contain a requested table or a missing-value
statement. The reviewed plan currently expresses those requirements in prose.

| Option                                                           | Product behavior and required work                                                                                                                                                                                                                                                                                                                                            | Meaning of the existing gate                                                                                                                                                                 |
| ---------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Structure the reviewed plan and assemble the document in the app | Add explicit table columns, requested fields and unavailable/unknown states to the plan the user reviews. Request source-linked typed values from the provider, validate them, then render tables and requested missing-value notices deterministically. This changes the plan/editor/storage contracts and still needs real 1.7B extraction and native export qualification. | Keep the original facts, table, absence, citation and reopen assertions. A pass would qualify the combined model-extraction and app-assembly path, rather than unconstrained model Markdown. |
| Redefine the release gate                                        | Explicitly decide which model capability the release requires and how unsupported generation is reported. Keep the existing failed case and model identity visible; do not describe a schema-only or graceful-failure check as successful document generation.                                                                                                                | Removing the table or absence assertions would change the requirement, not fix this observed failure. It requires an explicit policy decision.                                               |

Deterministic assembly must be generic, driven by the reviewed plan and current
evidence. It must not insert this fixture's expected values or reservation text
into a prompt, response or export. A missing extracted value does not prove its
absence from the original document: distinguish an explicit source statement
from information not found in the supplied excerpts, and keep that scope visible.
Preserve citations, unverified classifications, user edits and immutable
revisions. Raw model output and the rendered document remain separate evidence.

The selected implementation direction is the reviewed-plan/app-assembly path;
the original required 1.7B gate and its assertions remain in place. Its first
backend step stores an optional `DocumentPlan.assembly` contract with a version,
bounded field definitions, section assignments and field/table presentation.
Legacy plans omit that property and retain their existing JSON shape. The
contract contains no expected evidence values. Restart and revision restoration
preserve it exactly. Body generation with this contract requests typed values
instead of model-written Markdown, then assembles the reviewed fields and tables
in the backend. Each requested field must occur exactly once. The provider returns
literal values or quotes; the app assigns citations after finding one unique
match in the currently transmitted document excerpts. Values and explicit
unavailability require this match; information not found in the excerpts
has no citation and receives a scoped, unverified notice only when the reviewed
field requests one. These states remain distinct. Values are escaped as literal
text before adding table delimiters and source markers. Existing free-form plans
and partial rewrites retain their original path. Backend tests cover the new
contract. The outline editor exposes fields, two-column tables, row order and
missing-notice options in all five UI languages. Its production-preview check
uses fixture IPC to verify review invalidation, save payloads, reopening and
read-only history at desktop and narrow widths. This is client editing evidence;
actual-app 1.7B generation/export/reopening qualification remains pending.

A direct component probe on 2026-11-13 used the committed `69a602f` Rust
schema, extraction prompt and renderer with the original pinned 1.7B Q8 model
and b11960 CPU runtime. The manually reviewed layout contained field definitions
and column labels, without expected evidence values. The six controlled excerpts
and original facts/table assertions were unchanged. One actual request returned
the four correct values with their individual source labels, and classified the
reservation field as `explicitlyUnavailable` with its current excerpt citation.
The assembled document passed both original assertions. Temperature 0, 8192
context, 2048 output tokens and the existing resource/deadline guards were kept;
HTTP took 22.00 s (849 prompt / 222 completion tokens, normal stop, no truncation).
Minimum available memory was 1.79 GiB, with no guard stop; the owned server exited.

This is one combined extraction/assembly component result, not repeated stability
or the native release gate. It bypasses the app/editor/retrieval orchestration and
does not qualify PDF/HWPX export or reopening. Raw provider content and rendered
Markdown are separate, with no response repair. Evidence is retained in
`.dev/document-assembly-inference-20261113/qualification.json`, SHA-256
`db0e7ab5a5bdf582905c3f41dbd7df963294eebccce44edb9a9e2ccd69054283`.
The original required gate remains unresolved until the actual app path passes.

A subsequent field-ID-only probe used UUID-v4 IDs, matching the real editor.
Reversing those IDs restored the earlier request bytes exactly; the model,
runtime, excerpts, decoder and generation controls were unchanged. The four
values and the assembled facts/table checks passed, but the place value cited
the date excerpt `[S1]` instead of its supporting place excerpt `[S2]`. That
component result is **failed**, not an additional quality pass or evidence of
systematic UUID causality. Its raw content is preserved in
`body-wrong-field-source.txt`, used only by regression tests, never as a model
input. Full evidence is in
`.dev/document-assembly-uuid-inference-20261113/qualification.json`, SHA-256
`071009ea6156a69bcd27b757d84a79a39fc534dbf588f96d200e96ca6477866b`.

The `0d15e44` backend retained the exact transmitted, redacted and truncated excerpts
only for the current document operation. They were neither serialized in the API
answer nor stored in history. Provided field text had to occur in every cited
excerpt after whitespace normalization. An unrelated but known label is rejected,
without reassigning it or changing the model response. The captured UUID response
regresses this case, and a service test checks that rejection preserves the prior
revision. This is literal-span validation; field meaning, explicit-unavailability
classification and overall claim truth remain unverified. The stricter path
was subsequently tested with the actual model below.

A fresh request using that stricter source rule produced the same decoded wrong
place citation and was rejected by the newly compiled strict core. It was not
accepted or repaired. The same UUIDs and excerpts were retained; only the generic
literal/citation instruction was added to the request. Evidence is in
`.dev/document-assembly-strict-uuid-inference-20261113/qualification.json`, SHA-256
`39b0797fa6bfbbeb0ee42e8b3791f616bf8568edb90e1da29f556b8fdfc21218`.

A separate value/quote-only prototype then removed provider-owned citation
labels from its response contract. The app assigns a citation only when the
returned literal value or supporting quote uniquely matches a currently supplied
excerpt. Zero or multiple matches fail without guessing. An explicit
unavailability notice displays the actual source statement, rather than adding
a stronger absence assertion. Old `sourceLabels` responses are rejected as an
invalid schema, not silently corrected. Seven offline protocol checks cover
ambiguity, quotes, unsupported values, contradictory states and blank display.

One actual request with the same failed UUIDs and excerpts passed this prototype:
the original facts/table assertions passed, values were preserved and every
application-assigned citation matched its supporting excerpt. HTTP took 30.80 s
(1061 prompt / 391 completion tokens, cache 0, normal stop, no truncation); minimum
available memory was 3.79 GiB, with the original guards and owned-server cleanup
intact. The prototype also specializes the trusted system instructions to make
citations app-owned while preserving source-permission and untrusted-data rules.
This changes the extraction protocol, not just prompt placement. That prototype
did not qualify the native app, and one request is not a stability result.
Raw provider data and app-derived citation/rendering structures
are separate. Evidence is in
`.dev/document-assembly-derived-citations-20261113/qualification.json`, SHA-256
`fea384a63ccd37ecc47db8491b36531ec2c1fe7161f2230125408d11a61c6553`.

The production implementation now uses this value/quote-only contract. A typed
internal value carries app-owned citation metadata directly to the renderer;
the provider response is not rewritten into another model response. Old
`sourceLabels` payloads fail schema validation. An explicitly unavailable field
displays its literal supporting statement; a not-found field gets only the
reviewed, scoped notice or blank. Matching normalizes whitespace without joining
separate words or digits and rejects zero or multiple excerpt matches. This
checks literal support, not field meaning or claim truth.

An offline negative probe found that plain substring matching accepted `7명`
inside an excerpt containing only `17명`. The renderer now rejects values or
quotes clipped inside an ASCII digit run, both when checking a value against its
quote and when locating evidence in an excerpt. It still permits a later full
occurrence and ordinary Korean date suffixes; it does not interpret dates, units
or field meaning. RAG QA passed 131 tests, and the external-provider fixture's two
offline tests and both strict Clippy checks passed. The compiled new resolver
rejects that preserved negative case and renders the earlier positive actual
response identically; the decoder request is unchanged. This is captured-response
regression evidence, not a new inference or native app pass. Evidence:
`.dev/document-numeric-span-probe-20261114/qualification.json`, SHA-256
`4f4ed817e90b7d72d638cbe85fa07f8b97227dbcfa1636df967e359c6cca9d16`.

Only structured full-body generation receives the trusted citation-ownership
system specialization, and its length counts against the context budget. The
additional validation buffer of exact redacted/truncated excerpts is ephemeral,
excluded from API serialization, debug output and history, and discarded when
external evidence is revoked. Model output can include source quotations and is
stored under the existing answer/provenance controls; this buffer restriction
does not mean generated quotations are never saved.
Document revision citation-use flags reflect the assembled blocks; raw provider
turns retain their own citation metadata. Ordinary chat, legacy free-form plans,
outline generation and partial rewrites retain their existing paths.

On 2026-11-14, the `4371494` production prompt, schema and renderer passed a fresh
actual 1.7B request with the same UUIDs and excerpts. The request messages matched
the successful prototype; the only decoder-schema difference was an explicit
`type: string` on the state enum. Original facts/table checks, all five app-assigned
sources, explicit-unavailability classification and section coverage passed.
HTTP took 31.07 s (1061 prompt / 391 completion tokens, cache 0, normal stop,
no truncation). Minimum available memory was 3.49 GiB, original guards remained
active and the owned server exited. RAG QA passed 130 tests and strict Clippy.
`body-derived-citations.txt` preserves the earlier prototype response for decoder
and source-assignment regression tests only; it is never a model input.

That request was a component result, not repeated stability or a native app,
retrieval, PDF/HWPX export or reopen pass. The later native result follows below.
Evidence is in
`.dev/document-quote-production-inference-attempt2-20261114/qualification.json`, SHA-256
`0b9443b4b3735553c34f2c29ea579b8866c223d699cbd57e73ee97178f117c2b`.

A subsequent full native run of `42ee308` did not pass. The four recorded values
were correct, but the model used `explicitlyUnavailable` with a nonempty `text`
and empty `quote`. The renderer rejected this contradictory state and preserved
the reviewed revision. It was a normal model stop (1708 prompt / 361 completion
tokens), with no memory/deadline stop or console errors/warnings. Source, app and
asset hashes remained unchanged. The exact response is preserved in
`body-unavailability-in-text.txt` for regression tests only, never as a model
input. The failed native qualification is recorded in
`.dev/native-numeric-42ee308-resume-20261114/native-failed-qualification.json`, SHA-256
`bbac46c2cd6f08279fe2b58b346d11226b7e83cd3000182bfac46a910cfcdab7`.

The decoder schema now expresses the existing state rules as three disjoint
`anyOf` object variants. A provided value needs nonempty `text`; explicit
unavailability requires empty `text` and nonempty `quote`; not-found requires
both strings empty. The backend continues to reject the original malformed
response, without moving its text into another field. Prompt wording, response
keys, source matching, plan/storage formats and provider defaults are unchanged.
The pinned [b11960 converter](https://github.com/ggml-org/llama.cpp/blob/b11960/common/json-schema-to-grammar.cpp)
has union and constant grammar rules. Offline JSON Schema validation confirms
that the earlier flat schema admitted this exact native response and the new
schema rejects it. Synthetic valid protocol examples render identically in both
compiled renderers; these examples are not new model responses or inference
inputs. RAG QA passed 132 tests; external-provider offline tests passed two, and
both strict Clippy checks passed. Offline evidence:
`.dev/document-state-schema-compare-20261114/qualification.json`, SHA-256
`bbd2aa41639169d83a34cbb01ec0f0d996a32bced5a838e8481938709456e2ef`.

The Windows release app at `ead2af3` then passed the unchanged required actual
1.7B native gate. It used fresh private portable data, the original pinned models
and runtime, temperature zero and the original context/output limits. The real
editor review, source-based answer, schema-constrained body, requested two-column
table, HWPX export and reopening/answering all passed. The raw stored body matches
the captured response: all five field IDs match the reviewed plan, and the
explicit-unavailability value has empty `text` and a nonempty literal `quote`.
There was no response replacement, field repair, retry or assertion relaxation.
The body used 1701 prompt / 359 completion tokens, with 133 cached prompt tokens.
The full native run took 151.05 s, with a 3.06 GiB minimum available memory, no
resource/deadline stop, 82 unchanged integrity records, no console errors/warnings
and normal app exit. The generation server was stopped during owned cleanup.

A separate native IPC check exported that same immutable generated revision to
PDF without another inference. The original facts oracle passed; the one-page
PDF and its independent PyMuPDF rendering were checked for the date, place,
budget, headcount, table, literal absence statement and five source footnotes.
The complete rendered page had no observed clipping, overlap or missing glyphs.
The revision remained unchanged and the app exited normally. This supplement
qualifies the exporter, not the OS save dialog. Initial helper selector failures
were preserved and corrected before the successful checks.

The qualified executable is SHA-256
`a72fb0d9e161955c24bc4af06ed66dbd47d71061756474906f7441cb43d7500d`.
Combined native/PDF evidence:
`.dev/native-state-schema-ead2af3-20261114/qualification.json`, SHA-256
`7f6a5b03d68f8c12b49b57145937858548b3d283a8d0ae55831fc858e01328c8`.
This is one actual fixture success, not repeated stability, arbitrary-document
correctness or latest-source cross-platform CI qualification.

The later editor scroll fix at `e165f40` first failed its regression on the old
UI, then passed production-bundle browser checks. It was also verified in the
Windows release app at `290cac9`, using a clone of the actual generated document's
private portable profile. At 1600×1050 and 760×900, switching from the bottom of
the outline to body blocks showed the first generated value; keyboard navigation
back to the outline reset its viewport and preserved tab focus. The revision
remained unchanged, no horizontal overflow or app errors/warnings were observed,
and the owned app exited normally. An initial helper run omitted the portable
marker and stopped at the install-kind assertion before document interaction;
the corrected run verified the marker and expected scope/settings first.

The fixed UI executable is SHA-256
`a5025a2723b41e5b7367ea356b685db9f4f49ec0276e3c90b915ef62f9938395`.
All Rust sources and canonical native-generation scripts match qualified
`ead2af3`; the UI run made no additional inference and does not replace the
earlier model result. Native UI evidence:
`.dev/native-editor-scroll-290cac9-20261114/qualification.json`, SHA-256
`0676e3e8cfc868003e3c84eed488fdee7c4b656bd6d233c905e677934f40781f`.
Latest-source cross-platform CI and the whole-catalog 1 CPU / 512 MiB performance
target remain open.

After the filename-catalog hash-cache change, the `254a43f` Windows release app
also passed the unchanged required actual 1.7B native gate. Its source/app hash,
fresh private profile, original assets and 82 integrity records were verified.
The run ended at reopening the generated document and answering from it, with
no response replacement, assertion relaxation, resource stop or console errors/
warnings, and normal app exit. It took 150.91 s with 3.15 GiB minimum available
host memory. The actual stored body had 1698 prompt / 356 completion tokens,
133 cached prompt tokens, the reviewed field IDs and a literal unavailability
quote with empty `text`.

The same immutable revision passed native PDF export and the original facts
oracle without another inference. Its complete one-page PyMuPDF rendering was
reviewed for readable Korean, the requested table, values and five source
footnotes; no clipping, overlap or missing glyphs were observed. The actual
native screenshots were also reviewed; their first viewport shows the expanded
reference/options panel, while the exported PDF shows the complete generated
content. The new executable is SHA-256
`dbe35c262c555f67a8d772c2405606316e2b606b2bf3bb4bc828f71d766a9c71`.
Combined evidence:
`.dev/native-cached-hash-254a43f-20261114/qualification.json`, SHA-256
`dd1df0d7d62da288c684f9ba8d58b7bc0318a3ddc61698fdc0d0072320758435`.

This actual-model run used normal host resources. The separate
[app/WebView budget check](../../../tools/filename-qa/README.md#actual-app-and-webview-budget-check)
observed 5–7-second filename completion in both candidate starts inside a
one-CPU/512 MiB app job; the installed service stayed outside it, and no HDD was
tested. It does not claim the optional generative model runs inside that budget.
The original whole-project scope, latest-source cross-platform CI and physical
low-end limitations remain open; these results do not replace that audit.

Rejecting an output that violates a structured plan is a useful intermediate
behavior, but it does not satisfy the positive native generation gate. The
preserved failed free-form response contained neither requested output, so it
does not support relaxing the assertions. The required 1.7B fixture, original
checks and product provider defaults remain unchanged.

`outline-duplicate-ids.txt` retains the assistant's JSON content captured from
Qwen3-1.7B Q8 with the pinned b11960 CPU runtime on 2026-11-12. The direct wire
probe used the workshop document's paragraphs 6, 3, 5, 4, 1 and 9, separately
labelled as source excerpts. Both the deprecated thinking keyword and explicit
`--reasoning off` produced this same valid JSON with three identical IDs. This
is a structural-rejection regression, not a replacement for inference or proof
of the exact failed Windows CI request. The backend must reject duplicate IDs
and preserve its prior revision even when a decoder schema is enabled.

`outline-unique-ids.txt` is the follow-up content from the same model, schema and
six excerpts after the production prompt explicitly required distinct IDs.
It uses five distinct section IDs. This demonstrates valid structure, not full
instruction following: the document purpose requested two sections. Human
outline review remains required, and these direct probes do not replace the
full native chat/document generation gate.

`body-copied-headers.txt` retains a 2026-11-13 direct wire response from the same
pinned model/runtime with temperature 0, reasoning off, 2048 output tokens and
8192 context. The six current workshop excerpts and reviewed two-section plan
were kept; an unrelated recipe example illustrated table/source/missing-note
formatting. The model copied its 재료/분량 header into the workshop table despite
using the current amounts. It also omitted the requested missing-information
note. This failed experiment was not adopted in production. The capture tests
only the independent header assertion and is never supplied as an answer or
source by the actual native generation gate.

`body-echoed-missing-note.txt` retains one block from a 2026-11-13 direct
two-stage layout-contract prototype using the same pinned model/runtime. The
model repeated the user's instruction to state missing reservation information
instead of answering it. The old absence regex accepted that instruction echo.
The fixture records the full response file's SHA-256 and the unchanged block;
it tests rejection of that false positive and is never a generation input.
The prototype also chose incorrect columns and violated block identities. It
was not adopted in production and does not qualify the actual native gate.

## Separate experimental 4B qualification

The required 1.7B greedy checks and preserved failed runs remain unchanged. The
native program now reviews the structured layout through the actual editor;
facts, tables, missing-information, export and reopen assertions are retained.
The optional profile preparer pins this reviewed canonical program, and its
configuration-only reversal and JavaScript syntax checks pass. Earlier 4B
qualification results describe their recorded source revision; they do not
qualify this new editor flow. A separate
opt-in profile can be prepared with Python 3.11 or newer, without downloading
assets or launching a model:

```sh
python -B scripts/prepare-generation-qualification.py --output .dev/qwen4b-qualification
```

The new directory contains a test tree and `qualification-profile.json` with
exact commands and input hashes. Its asset preparer is an unchanged copy of
`scripts/prepare-generation-assets.mjs`, reading the generated tree's separate
pins. Run that preparer only when explicitly preparing the experiment. Map its
`assets.json` paths to the original `PRISM_REAL_GENERATION`,
`PRISM_REAL_EMBEDDING` and `PRISM_REAL_RUNTIME` variables. Install the repository's
locked Node dependencies first; the generated tree remains inside `.dev` so
the native harness resolves them from the repository.

This profile uses pinned Qwen3-4B Q4_K_M, thinking on, `--no-repack`, temperature
0.6, top-p 0.95, top-k 20, min-p 0, presence penalty 1.5 and seed 42. The 8192
context, 2048 total output budget, source fixtures, facts/table/citation/history
assertions, 180-second HTTP deadline and original memory guards stay intact.
Only the named model/server/fixture settings change; reversing those changes
reproduces the canonical original program. Preparation fails closed if that
program changes, requiring a fresh review before updating the pinned hash.
Captured negative answers are not copied as experiment inputs.

Execution still requires Windows x64 and the original explicit release-app
path/SHA/source-commit environment. Use a fresh private profile and an external
15-minute whole-case timeout (for example a CI step timeout). Run the generated
test's `--verify-assets` mode before its actual inference mode. Preparation and
independent Markdown checks are not an actual-app or HWPX qualification pass.
The profile changes neither product defaults nor the original required gate.
