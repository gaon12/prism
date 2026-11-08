# Actual local chat and document generation

This **opt-in Windows x64** gate uses a fresh ordinary release, actual pinned CPU
models, the real file extractor, SQLite/Lance search and saved chat/document
services. It never replaces inference responses, retrieval results or model
status. It does not download anything, install the filename service, read user
folders, change credentials or contact an external generation provider.

The flow is: fresh portable profile → import authenticated embedding/runtime
assets → actual RAG chat → enable local document JSON Schema in settings → model
outline → explicit human outline review → actual model body → Markdown export →
restart and read the same saved revision and answer.

The fixed public sources include a workshop, a conflicting seminar and an
unrelated book club. Both the answer and document must preserve the workshop's
date, location, budget and attendance, cite its source and acknowledge its absent
reservation number. Model prose remains unverified. Human review edits only the
outline's structure and instructions; it does not insert the expected facts or a
replacement answer. The original model outline is preserved separately.

## Inputs

Obtain the exact files in [pins.json](../fixtures/generation/pins.json), respecting
their upstream licenses. Qwen3-1.7B Q8 is a test-only generation model, not a new
product default. EmbeddingGemma 2 Q8 and the b11960 CPU archive match the app's
immutable inventory. The test hashes every supplied asset before use and rehashes
the input files and installed runtime after cleanup.

Authenticate the candidate release against its CI build/artifact provenance
before running. `PRISM_TEST_APP_SHA256` pins that selected executable; setting it
does **not** itself establish publisher signature or artifact authenticity.

PowerShell example, with real paths substituted:

```powershell
$env:PRISM_REAL_GENERATION = 'D:\qa-assets\Qwen3-1.7B-Q8_0.gguf'
$env:PRISM_REAL_EMBEDDING = 'D:\qa-assets\embeddinggemma-2-Q8_0.gguf'
$env:PRISM_REAL_RUNTIME = 'D:\qa-assets\llama-b11960-bin-win-cpu-x64.zip'
$env:PRISM_TEST_EXECUTABLE = 'D:\verified-release\prism.exe'
$env:PRISM_TEST_APP_SHA256 = '<verified executable SHA-256>'
$env:PRISM_TEST_EXPECT_RELEASE = '1'
$env:PRISM_TEST_SOURCE_SHA = '<40-character CI source commit>'
# Use a drive with space for a fresh app/profile, models and evidence.
$env:TEMP = 'D:\qa-temp'
$env:TMP = $env:TEMP
node tests/native/real-generation.mjs
```

The full run requires **4GiB available physical memory before execution**. A
250ms guard stops only this test's app/server handles if available memory falls
below **1GiB**. Do not substitute a smaller model, shrink the 8192-token server
context, lower the reserve, inject factual answers or relax assertions to obtain
a pass. Product retrieval defaults remain `topK=null` and
`maxContextChars=null` (effective 6 and 8000). The server uses a single CPU slot,
four threads, temperature zero and disabled Qwen thinking. Product transport
deadlines remain unchanged; slow or inaccurate model output is a failed gate.

To verify only the three existing input assets, without launching an app/server
or doing inference:

```powershell
node tests/native/real-generation.mjs --verify-assets
```

A successful asset-only result has `verificationOnly=true` and
`actualInference=false`; it is **not** a native UI, schema or generation pass.
It does not require an executable or the inference memory reserve.

## Ownership and evidence

The runner copies the selected app beside a portable marker and creates one
unique fixture scope. Before mutation, model commands, screenshots or cleanup
IPC, it verifies that scope's nonce/path. On an unverified CDP endpoint it only
detaches and terminates its own spawned child. Clipboard/OCR/autostart/update
checks, external access and fallback are disabled. No development override is
passed to the ordinary portable app.

Browser plugin not available: Playwright connects to the actual bundled
WebView2 over loopback CDP. The runner uses native app IPC for observation and
real UI controls for chat, schema opt-in, outline review and body generation.
It checks both windows' JavaScript errors, the 760px document layout, explicit
review gating, immutable prior revisions and persistence after restart.
Screenshots prove only a run that actually reached their recorded stage; review
them before claiming visual QA.

Each invocation keeps its own `prism-real-generation-*` directory, including
failed/preflight cases. `evidence/result.json` records the reached stage,
resource stop, identity, model outputs, citations and revision IDs. An inference
failure retains the previous revision; inaccurate but structurally accepted
output remains in the failed profile for diagnosis. Frozen copies of this
runner, its shared harness, pins and source fixtures are under `evidence/inputs`.
The result rechecks their original and frozen hashes as well as asset/runtime
integrity. A cleanup/resource/integrity failure invalidates a prior success.

On a disposable elevated CI runner, use the workflow owner's existing WebView2
debug policy pattern for **prism.exe**, with a dedicated free
`PRISM_APP_CDP_PORT`; the runner will reject an occupied endpoint. Do not create
that policy on a user's machine. Upload only the invocation's `evidence/**` with
an always-run artifact step, not its entire profile, models or an arbitrary temp
directory. The workflow is maintained separately from this runner.

This gate does not prove all model accuracy, external providers, PDF/HWPX
rendering, physical clipboard/paste, macOS/Linux native behavior, production
update/install, or the C-drive filename performance target. Those retain their
own requirements and evidence.
