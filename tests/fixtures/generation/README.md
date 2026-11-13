# Actual-generation fixtures

The three Markdown files are public, test-authored source documents. The native
generation gate checks actual pinned models against their dates, amounts and
locations; it does not substitute generated answers.

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
