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
