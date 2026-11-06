# Office preview worker (integration in progress)

This internal worker converts authenticated DOCX, PPTX and XLSX bytes into PDF
bytes using a real LibreOffice layout engine. It is the first part of the
collection original-page preview work; the application does **not** call it yet.
There is no renderer download UI or production runtime package yet. Windows x64
is the initial execution target. Native Linux/macOS execution remains pending.

The private QA engine is LibreOffice 26.8.2 from the official Windows x64 MSI,
375,934,976 bytes, SHA-256
`ff4b911403b67ee9e6249f820f9782c44bb32c94fdaa45b0da53a8e419e5307a`.
It was extracted without running Windows Installer or changing registration.
The engine itself is not vendored here. Its licenses, notices and source-code
availability must accompany any future distributed runtime.
This QA copy uses the extracted LibreOffice application tree; redistribution
must also account for the MSI's separate font and VC runtime components. Passing
on this development machine, which already has Office fonts and VC runtimes,
does not establish a self-contained package for a clean Windows installation.

The parent must verify the runtime and worker before executing the bundled
Python interpreter directly, in isolated mode (`-I -S -B`). Do not launch its
Python wrapper: the actual interpreter must be the owned process. Supply the
runtime directory and a fresh private work directory containing `owner.txt`
with the exact bytes `Prism Office preview worker v1\n`. No document path or
password belongs in the command line.

Input is one UTF-8 JSON line (at most 4096 bytes) with exactly `version: 1`,
`extension: "docx" | "pptx" | "xlsx"` and `bytes: N`, followed by exactly N
document bytes. N is bounded to 64 MiB. **Keep stdin open** until the worker
exits. EOF or a control byte after the body cancels the worker and its engine.
The parent must also enforce a deadline, bound stdout, check the final child
exit, and discard output after cancellation or source/permission changes.

Output is a JSON line followed by `pdfBytes` PDF bytes on success, or a fixed
error code without a body. The PDF bound is 64 MiB. Never surface native UNO
exception messages as diagnostics. The parent still has to validate PDF pages,
render bounded pixels and encode WebP before publishing collection media.

The worker enters a private Windows kill-on-close Job Object **before** it can
start LibreOffice. Its non-inheritable handle lives until worker-process teardown;
later descendants inherit membership. This also bounds job memory to 2 GiB. A
stdin lifeline covers parent death during startup. A private LibreOffice profile
prevents attaching to a user's existing instance. `InputStream`/`OutputStream`
carry document/PDF bytes; the worker does not create a document copy on disk.
This is not a claim that LibreOffice never uses internal temporary files, nor
that a Job Object is a file/network sandbox. Any such internal files must stay
under the private work directory and be cleaned by the future parent.

Each load explicitly sets `MacroExecutionMode` to `NEVER_EXECUTE`, `UpdateDocMode`
to `NO_UPDATE`, read-only/hidden/silent operation, no recent-document entry and
an interaction handler that selects only abort. These use the
[official document properties](https://api.libreoffice.org/docs/idl/ref/servicecom_1_1sun_1_1star_1_1document_1_1MediaDescriptor.html).
The engine uses a separate profile as documented in
[LibreOffice startup options](https://help.libreoffice.org/latest/en-US/text/shared/guide/start_parameters.html).
They do not replace process/resource containment or verification with actual
documents, external-reference canaries and cancellation tests.

## Developer verification

The protocol checks require only the developer's Python standard library:

```powershell
python -B -m unittest discover -s tools/office-runtime -p test_worker.py -v
ruff check tools/office-runtime
ruff format --check tools/office-runtime
```

Actual execution requires Windows, the explicitly supplied extracted runtime
matching `windows-source.lock.json`, and developer-only `pypdf`, `pypdfium2`
and Pillow packages. They are not application dependencies:

```powershell
$env:PRISM_OFFICE_QA_RUNTIME = 'D:\path\to\private\extracted-runtime'
python -B tools/office-runtime/test_actual.py
python -B tools/office-runtime/test_external.py
```

The first test verifies the repository's independently sourced public DOCX,
PPTX and XLSX fixtures against their pinned hashes, renders their actual PDFs
with a separate PDF engine, writes first-page WebP images in an owned temp
directory, and compares source hashes again. It opens concrete live process
handles before EOF, explicit cancellation and actual hard-parent-death tests;
all engine descendants must terminate while an independently spawned canary
stays alive. No process is terminated by an enumerated PID or name.

The second test adds a local external-image relationship without rewriting the
source document's namespace declarations. It first proves its loopback canary
server is reachable, then verifies successful PDF conversion with zero image
requests. This tests that case only; it does not establish a universal network
restriction or support for every external-link/document feature.

Application integration still requires verified runtime inventory/downloads,
source fingerprint and permission rechecks, collection page identities and
cache invalidation, native PDF-to-WebP publication, user-visible progress,
native Linux/macOS lifetimes and tests, and broader document/layout coverage.
The existing application continues to label Office text extraction as a text
preview; these standalone engine checks must not be presented as a working
collection UI or as Word/Excel/PowerPoint pixel-equivalence.
