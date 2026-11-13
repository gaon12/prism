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
must preserve it exactly. Typed extraction and rendering are not connected in
this initial step: body generation with an assembly contract fails closed rather
than silently using the existing free-form Markdown generator. There is no new
editor entry point or actual-model qualification from this groundwork alone.

Rejecting an output that violates a structured plan is a useful intermediate
behavior, but it would not satisfy the current positive generation gate. The
present response contains neither requested output, so there is no semantic
false-negative evidence supporting an assertion relaxation. The required 1.7B
fixture, original checks and product provider defaults remain unchanged while
these alternatives are reviewed.

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

The original 1.7B greedy gate and its failures remain unchanged. A separate
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
