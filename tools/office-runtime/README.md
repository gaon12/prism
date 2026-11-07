# Office preview worker (integration in progress)

This internal worker converts authenticated DOCX, PPTX and XLSX bytes into PDF
bytes using a real LibreOffice layout engine. It is the first part of the
collection original-page preview work. The collection service now calls it when
the pinned managed engine is installed at the application's `office` data path.
There is no renderer download UI or published runtime package yet. A pinned
Windows x64 package and Rust transport are tested independently of the app.
Native Linux/macOS execution remains pending.

The private QA engine is LibreOffice 26.8.2 from the official Windows x64 MSI,
375,934,976 bytes, SHA-256
`ff4b911403b67ee9e6249f820f9782c44bb32c94fdaa45b0da53a8e419e5307a`.
It was extracted without running Windows Installer or changing registration.
The engine itself is not vendored here. Its licenses, notices and source-code
availability must accompany any future distributed runtime.
The managed QA package includes the MSI's separate fonts and x64 VC runtime
components, plus the repository's licensed CJK/document fonts and their notices.
Passing on this development machine does not establish a self-contained package
for a clean Windows installation.

## Pinned package and Rust transport

`windows-x64.lock.json` pins the compressed archive, expanded inventory and
upstream MSI. Its URL is deliberately `null`: this package is unpublished, and
there is no automatic download. The archive is 544,426,755 bytes; its 19,426
files expand to 1,620,519,547 bytes. `package_windows.py` is a maintainer tool:
an MSI hash does **not** authenticate an arbitrary extracted source folder.
Use an independently verified, untouched extraction of that MSI.

MSI merge modules have colliding x64/ARM64 long filenames. Select the ten VC
runtime CAB members by their original `*_amd64.<guid>` File identifiers, rather
than using a flattened `System64` directory. The builder checks their PE machine
type, the engine/interpreter architecture, notice files, inventory and every
archive member. Supply that separately extracted CRT directory with `--crt`:

```sh
python3 -B tools/office-runtime/package_windows.py \
  --msi /private/LibreOffice_26.8.2_Win_x86-64.msi \
  --extracted /private/verified-msi-extraction \
  --crt /private/original-amd64-cab-members \
  --output /private/new-runtime.zip --lock /private/new-runtime.lock.json
```

The Rust modules `office_assets.rs` and `office_preview.rs` verify the compiled
catalog and every engine file,
reject additional modules, install through an owned staging directory, and
exchange bounded document/PDF bytes with the worker. A short Windows rename
retry handles transient sharing violations without replacing a destination or
changing permissions. A final promotion error retains the verified stage;
other installation failures and cancellation clean only the owned stage.

Rendering clears the inherited environment and creates a private request
directory. Cancellation/deadlines close the owned worker, wait for its pipe
threads and clean that directory. The engine's separate embedded Python also
receives no-user-site/no-bytecode settings and a private cache location. Merely
passing `-I -S -B` to the worker does not configure that second interpreter.
There is no crash-recovery sweep of stale request directories yet.

Private request names retain 128 bits of randomness while leaving space for
LibreOffice's deep extension-registry paths. The real app previously reached
263 characters and crashed during registration; the unchanged worker succeeded
with the same engine/document and a 225-character path. The engine receives
`-env:CrashDumpEnable=false`, and inherited `CRASH_DUMP_ENABLE` is removed.
LibreOffice otherwise initially writes dumps into its current directory before
the profile location is ready; a crash could therefore dirty the sealed engine.
These switches follow the
[pinned LibreOffice crash-report implementation](https://raw.githubusercontent.com/LibreOffice/core/libreoffice-26.8.2.2/desktop/source/app/crashreport.cxx).
Integrity verification continues to reject every extra file.

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
under the private work directory; the Rust parent cleans its owned directory
after normal completion or cancellation.

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
python -B -m unittest discover -s tools/office-runtime -p test_package.py -v
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
python -B tools/office-runtime/test_crash_guard.py
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

The crash-guard test uses a deliberately long private profile and a real engine
startup failure. An inherited dump-enable override is set in the test process;
the worker must discard it, return its fixed error, and add no dump or other
file to the installed engine. It does not inspect crash-memory contents.

The standalone Rust QA crate imports the exact application modules without
linking the full desktop app. Its `install` command requires a new private data
directory and the archive pinned by the compiled catalog; it never installs
system Office or changes registration:

```powershell
cargo test --manifest-path tools/office-runtime/qa/Cargo.toml --locked
cargo build --manifest-path tools/office-runtime/qa/Cargo.toml --release --locked
$env:PRISM_OFFICE_QA_EXE = 'D:\private\target\release\prism-office-qa.exe'
& $env:PRISM_OFFICE_QA_EXE install D:\private\runtime.zip D:\private\new-qa-data
$env:PRISM_OFFICE_QA_DATA = 'D:\private\new-qa-data'
python -B tools/office-runtime/test_managed.py
```

`test_managed.py` exercises DOCX/PPTX/XLSX through Rust's cleared environment,
checks actual PDF text/pages with an independent parser and writes real WebP
rasters. A synthetic Korean two-page DOCX checks first/second-page identities.
It also cancels after the actual engine is alive, checks retained process handles
and an unrelated owned canary, and requires an empty work directory afterwards.
Creation times exclude stale PPID edges. A deliberate ancestry-only fixture
adds a real older canary as a candidate and requires its rejection and survival;
it does not replace the real renderer or engine. Lifetime failures report the
specific retained handle's process name and native wait result.
It retains private test outputs and hashes for review. These are core transport
checks, not tests of the desktop collection UI or a clean Windows machine.

One earlier managed run timed out waiting five seconds for an unnamed process
handle; inspection found no remaining engine/worker/CLI and an empty work
directory. Eight instrumented cancellation runs then ended all retained handles.
Creation-time guards and per-handle diagnostics improve the test, but do not
prove the historical failure's cause. Retain that failure alongside subsequent
passing results when reviewing the next application integration.

## Collection integration

`collection_office.rs` creates original-page manifests for DOCX/PPTX/XLSX with
the installed managed engine. Other formats and installations without that
engine retain the explicitly labelled extracted-text preview. Original Office
pages have a separate `office` kind and pin inventory/worker fingerprints in
their identities, so text pages and older engine thumbnails cannot be reused.
Existing non-Office page identities retain their serialization.

Generated PDFs stay in a serialized, bounded 128 MiB/eight-document RAM cache.
The native PDF renderer produces page pixels, then the existing encoder/cache
publishes WebP. Current original bytes and collection references are checked
again before publication, including when a WebP thumbnail is already cached.
Registration still grants no content-index, AI or external-transmission access;
the engine/work directory is excluded from content indexing.

Desktop IPC provides `collection_office_status`, `collection_office_install`
and `collection_office_cancel` for an explicitly selected, exactly pinned local
archive. Status `installed` describes presence only: execution always performs
full inventory verification. Installation progress uses `prism:office-runtime`,
and existing unit-import progress remains active while pages are rendered.
There is no public package URL or automatic download. A delivery/settings UI,
native Linux/macOS lifetimes, clean-machine dependency coverage, stale-work
recovery and broader document/layout coverage remain pending.

Opt-in collection verification uses only private data and the authenticated
archive. The native test accepts the synthetic two-page DOCX generated by
`test_managed.py` and checks its exact hash before launching anything:

```powershell
$env:PRISM_OFFICE_QA_ARCHIVE = 'D:\private\runtime.zip'
$env:PRISM_OFFICE_QA_TWO_PAGE = 'D:\private\managed-test-output\two-pages.docx'
$env:PRISM_TEST_EXECUTABLE = 'D:\private\target\debug\prism.exe'
npm run test:collections-office
```

The service also has an ignored opt-in real-engine regression test:
`real_office_pages_are_reference_only_and_cached_pixels_recheck_the_source`.
These commands must be run against the current compiled code; the standalone
worker qualification alone does not prove the desktop collection path or
Word/Excel/PowerPoint pixel-equivalence.
