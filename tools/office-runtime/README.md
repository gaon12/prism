# Office preview worker (integration in progress)

This internal worker converts authenticated DOCX, PPTX and XLSX bytes into PDF
bytes using a real LibreOffice layout engine. It is the first part of the
collection original-page preview work. The collection service now calls it when
the pinned managed engine is installed at the application's `office` data path.
There is no renderer download UI or published runtime package yet. Pinned
Windows x64 and Linux x64 packages, the Rust transport and collection page services
have actual tests. Linux execution was checked under WSL; Linux desktop/AppImage
and native macOS engine execution remain pending.

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
retry (at most two minutes) handles observed transient sharing violations without replacing a destination or
changing permissions. A final promotion error retains the verified stage;
other installation failures and cancellation clean only the owned stage.

Rendering clears the inherited environment and creates a private request
directory. Cancellation/deadlines close the owned worker, wait for its pipe
threads and clean that directory. The engine's separate embedded Python also
receives no-user-site/no-bytecode settings and a private cache location. Merely
passing `-I -S -B` to the worker does not configure that second interpreter.
Crash recovery for new, leased request directories is described below.

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
Python interpreter directly. Windows uses isolated mode (`-I -S -B`). Linux's
bundled interpreter has a relocatable, flat standard library, so it uses
`-P -S -B` with a cleared environment and explicit private `PYTHONHOME` and
standard-library-only `PYTHONPATH`. User startup/site/search paths are not
inherited. Do not launch its
Python wrapper: the actual interpreter must be the owned process. Supply the
runtime directory and a fresh private work directory containing `owner.txt`
with the exact bytes `Prism Office preview worker v2\n`. The worker holds a
shared lease on this marker. Explicit maintainer fixtures may still use v1,
which is never eligible for automatic recovery. No document path or
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

On Linux, `unix_owner.py` watches a worker-owned pipe in a separate supervisor
session and owns the engine's private process group. It resets inherited
`SIGCHLD` ignore, observes exit with `waitid(..., WNOWAIT)`, signals the group
while the exact leader remains unreaped, then reaps it and adopted descendants.
The Linux [subreaper](https://man7.org/linux/man-pages/man2/PR_SET_CHILD_SUBREAPER.2const.html)
attribute applies only to this supervisor. An exec helper sets per-process
limits before starting the engine: 2 GiB virtual address space, 64 MiB per
output file, 90 CPU seconds, 256 descriptors, and no normal core file. Existing
lower inherited limits stay lower. These are not aggregate Job Object limits
or a replacement for the Rust request deadline. Engine restart is bounded to
three attempts, only for the official
[normal-restart code 81](https://raw.githubusercontent.com/LibreOffice/core/libreoffice-26.8.2.2/include/desktop/exithelper.h).
Closing the worker pipe, worker death, or app death ends the owned group;
unrelated processes are never found or killed by name. A hard app death can
leave its marked private work directory until a later recovery pass.

## Request crash recovery

New request directories use a v2 ownership marker. Rust and the Python worker
hold independent shared file locks; the Linux supervisor retains another until
its engine descendants have been killed and reaped. Recovery needs an exclusive,
nonblocking lock on that same marker. An old timestamp cannot authorize removal
of a live or suspended request. Rust's standard file locks interoperate with
the worker's native `flock`/`LockFileEx` locks; see the
[Rust file lock contract](https://doc.rust-lang.org/std/fs/struct.File.html#method.try_lock)
and [Windows shared locks](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-lockfileex).

Before creating the next original-page rendering workspace, Prism considers
inactive v2 requests whose marker is at least 24 hours old. A cached-page read
does not start this pass. Candidates must have the exact `r-` plus 32 lowercase
hexadecimal name, resolve directly inside this runtime's private `work` root,
and contain the exact regular ownership marker. Legacy v1, unknown, future-dated,
linked or inaccessible directories are retained. Engine installation stages and
user originals are outside this sweep.

The pass checks cancellation and the caller's deadline, examines at most 512
root entries, and budgets 250 ms between filesystem operations. It validates
each tree before deleting anything, rejecting symlinks/junctions, and bounds
inspection to 4,096 entries and 64 levels. Oversized or blocked trees remain;
this does not promise unconditional reclamation of every stale directory.
Deletion keeps the marker until the last step, so an interrupted partial cleanup
can resume. Normal request cleanup also obtains the exclusive lease and uses a
five-second budget between operations. An individual OS filesystem call can
outlast these budgets. No process is discovered or terminated by cleanup.

`test_recovery.py` runs an actual managed conversion, kills only its held Rust
parent, and observes owned engine termination while an unrelated canary stays
alive. A subsequent real conversion must retain the recent leftover. The test
then ages only that newly observed marker, and a further real conversion must
remove it while preserving all previously retained work directories. This
simulates expiry; it does not wait 24 real hours. Run it with explicit
`PRISM_OFFICE_QA_EXE`, `PRISM_OFFICE_QA_DATA` and a new
`PRISM_OFFICE_QA_OUTPUT` directory. The data directory must contain the exact
maintainer QA marker used by `test_managed.py`/`test_linux.py`.

The actual Windows x64 and Linux x64 CLI checks passed this sequence, including
owned-process termination, recent retention, aged recovery and preservation of
the two pre-existing Linux v1 directories. Independent PDF parsing confirmed
the expected page and text in both follow-up conversions on each OS. The
Linux collection service also rendered the four real Office inputs into five
native WebP pages with source rechecks. These are local Windows/WSL results;
they do not qualify macOS Office, Linux desktop/AppImage or a clean machine.

## Existing Office previews after an update

Stored original-page manifests carry the engine inventory and worker identity.
`collection_unit_prepare` and cover loading rebuild outdated Office sources
through a cancellable job. The matching engine must already be installed.
Source hashes, current references, excluded paths and unit revisions are checked
before publication. A changed original is not silently adopted by this process.
All replacements publish in one SQLite transaction; failure leaves the previous
manifest and reading state intact.

Surviving physical pages retain the user's ordering, including interleaved
sources. New pages follow the last old page of that source. The transaction
remaps an Office bookmark only within the same unchanged original, preserving
its completed flag. It chooses the same physical page number or the nearest
remaining number when pagination shrinks. This does not establish an equivalent
text position after reflow. New renderer identities use new page/cache IDs;
old cached pixels never bypass the renderer or source checks.

The client retains unit metadata and explicit recovery actions if preparation
fails, while blocking old-page rendering. `prepareFailed` clears the active job
without triggering the view's automatic terminal reload, avoiding a retry loop.
Explicit refresh/reimport remains a user action and may adopt a changed original.

The Windows version-transition regression uses a qualified old app with worker
`0edc896a…` and a new app with worker `1822d414…`, both using the same pinned
LibreOffice inventory. The old app creates a real two-page Korean manifest,
reorders it to 2→1, saves a completed bookmark and generates a real cached cover.
The pre-change BE38 app reproduces the incorrect original-changed error. The
new app migrates that same library, preserves order/bookmark/completion and
renders the original pages. A changed second source is rejected without an
automatic retry loop; restoring the test-owned original and explicitly
reimporting it succeeds. This checks an actual worker-version transition, not
two different LibreOffice versions or native macOS/Linux desktop integration.

Run `npm run test:office-upgrade` with `PRISM_TEST_EXECUTABLE`,
`PRISM_OFFICE_UPGRADE_OLD_EXE` (the exact qualified 68B0EE app),
`PRISM_OFFICE_QA_PREPARED_RUNTIME` and the pinned `PRISM_OFFICE_QA_TWO_PAGE`
fixture. `PRISM_OFFICE_UPGRADE_EXPECT_BLOCKED=1` is the separate pre-change
negative control. New runs use fresh isolated data; a maintainer can explicitly
resume a marked, seeded fixture with `PRISM_OFFICE_UPGRADE_ROOT`. An already
migrated fixture is not a fresh transition test. The harness records actual
native events rather than replacing IPC or renderer responses. It uses a
prepared private runtime and is not ZIP-installation qualification.

## Linux maintainer package

`linux-source.lock.json` pins the official 26.8.2 DEB tar and private
NSS/NSPR/SQLite dependency archives. The dependency index was checked against
the signed Ubuntu Jammy-updates InRelease and its Packages SHA-256; the tool
does not install packages or run maintainer scripts. The supplied extracted
trees are trusted maintainer inputs, with the same caveat as the Windows tool.
`package_linux.py` materializes contained links, checks every ELF is x86-64,
and includes licensed fonts, source notices and dependency copyright files.
Its schema-2 inventory pins file bytes and 0444/0555 permissions. Installation
restores these modes and execution verifies them; writable, missing-execute or
setuid variants are rejected. The Windows schema-1 catalog remains unchanged.

```sh
python3 -B tools/office-runtime/package_linux.py \
  --upstream /private/LibreOffice_26.8.2_Linux_x86-64_deb.tar.gz \
  --runtime /private/extracted/opt/libreoffice26.8 \
  --dependencies /private/extracted-dependencies \
  --dependency-archives /private/verified-debs \
  --output /private/office-linux-x64.zip --lock /private/linux-x64.lock.json
```

The pinned private archive contains 13,983 files, 775,021,206 expanded bytes
and 341,071,196 compressed bytes. Both runtime catalog URLs remain null.
The dependency packages require glibc 2.34 or later; actual older-host and
clean-machine compatibility is not established by WSL execution.

Linux verification uses the exact-source Rust Office QA binary and an isolated
installed runtime. It covers DOCX/PPTX/XLSX and a Korean two-page DOCX, original
hashes, actual active-engine cancellation and parent SIGKILL with a surviving
unrelated canary. Independent PDF text/page/pixel inspection is separate.

```sh
python3 -B -m unittest discover -s tools/office-runtime -p test_unix_owner.py -v
python3 -B -m unittest discover -s tools/office-runtime -p test_package_linux.py -v
PRISM_OFFICE_QA_EXE=/private/target/debug/prism-office-qa \
PRISM_OFFICE_QA_DATA=/private/installed-qa \
  python3 -B tools/office-runtime/test_linux.py
```

The collection service opt-in test additionally uses the actual Linux CPU PDF
renderer to produce WebP, including Korean text, and rejects cached media after
source changes. A lib-test has no pre-GUI dispatch, so test-only
`PRISM_TEST_PDF_LAUNCHER` points to the exact-source platform QA host. Release
code cannot read this override and always uses its own executable.

Each load explicitly sets `MacroExecutionMode` to `NEVER_EXECUTE`, `UpdateDocMode`
to `NO_UPDATE`, read-only/hidden/silent operation, no recent-document entry and
an interaction handler that selects only abort. These use the
[official document properties](https://api.libreoffice.org/docs/idl/ref/servicecom_1_1sun_1_1star_1_1document_1_1MediaDescriptor.html).
The engine uses a separate profile as documented in
[LibreOffice startup options](https://help.libreoffice.org/latest/en-US/text/shared/guide/start_parameters.html).
The local Python/UNO connection is isolated **before importing `uno`**, because
that import immediately initializes its own component context. Both local UNO
and the renderer use the same request-owned `UserInstallation`. Configuration
layers contain only the verified engine's schema/defaults/resources and the
request's private overrides, excluding the host LibreOffice profile, extension
configuration and Windows HKCU/HKLM configuration layers. This follows the
pinned [bootstrap variable lookup](https://raw.githubusercontent.com/LibreOffice/core/libreoffice-26.8.2.2/sal/rtl/bootstrap.cxx)
and [local context initialization](https://raw.githubusercontent.com/LibreOffice/core/libreoffice-26.8.2.2/pyuno/source/module/pyuno_module.cxx).
It does not block every operating-system configuration source: OS fonts and
other normal platform services still exist, and this is not a network sandbox.
These settings do not replace process/resource containment or verification with actual
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
python -B tools/office-runtime/test_bootstrap.py
python -B tools/office-runtime/test_external.py
python -B tools/office-runtime/test_crash_guard.py
```

`test_bootstrap.py` verifies the pinned engine files, redirects `SYSUSERCONFIG`
to an owned canary, and gives that canary an invalid extension service registry.
The control must fail while reading that exact negative fixture. The production
bootstrap must initialize successfully, resolve its own extension cache and
read an explicitly seeded private locale, without a `winreg:` or canary layer.
The canary's files must stay unchanged. No normal user profile is initialized
or modified, and no UNO/renderer response is mocked.

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
Settings → General now includes **Original Office pages**, with archive/disk
sizes, an explicit local ZIP picker, installation progress and a separate
cancel action. The retained `collection_office_status` snapshot and revisioned
`prism:office-status` notifications include terminal failure/cancel states;
closing settings or switching tabs does not abandon an installation. Status is
retained for the app session, rather than persisted across process restarts.
Every ZIP must match the compiled catalog; an arbitrary LibreOffice ZIP is not
accepted. Reimport an original to replace its existing text-preview manifest.
There is no public package URL or automatic download. Package delivery,
native macOS lifetimes, Linux desktop/AppImage integration, clean-machine
dependency coverage, recovery of legacy/oversized work directories and broader document/layout coverage
remain pending.

Opt-in collection verification uses only private data and the authenticated
archive. The native test accepts the synthetic two-page DOCX generated by
`test_managed.py` and checks its exact hash before launching anything:

```powershell
$env:PRISM_OFFICE_QA_ARCHIVE = 'D:\private\runtime.zip'
$env:PRISM_OFFICE_QA_TWO_PAGE = 'D:\private\managed-test-output\two-pages.docx'
$env:PRISM_TEST_EXECUTABLE = 'D:\private\target\debug\prism.exe'
npm run test:collections-office
# Real settings progress, cancel, completion while closed and remount recovery:
node tests/native/office-engine-settings.mjs
```

The service also has an ignored opt-in real-engine regression test:
`real_office_pages_are_reference_only_and_cached_pixels_recheck_the_source`.
These commands must be run against the current compiled code; the standalone
worker qualification alone does not prove the desktop collection path or
Word/Excel/PowerPoint pixel-equivalence.

To isolate reader verification from package installation, the native test also
accepts `PRISM_OFFICE_QA_PREPARED_RUNTIME` pointing to an already authenticated
private runtime. This explicit mode copies it into a fresh isolated profile,
checks its inventory identity and runs actual text-to-Office reimport, pages,
WebP, navigation and source-change rejection. The production execution path
still verifies every engine file. Its result says
`installationTest: "notRunPreparedRuntime"`; it never counts as a ZIP install
or cancellation test. Screenshots stay outside the repository. The default
archive-install test remains mandatory and still fails if installation fails.

Windows can temporarily deny final directory promotion after a valid archive
has been extracted and verified. A real tree became movable 37 seconds after
the old short retry window, with its installer process still alive. Actual app
finalization also exceeded a one-minute window under heavier I/O. Promotion now
retries only native errors 5/32/33 for at most two minutes, while checking the
request deadline and cancellation. It never changes ACLs, closes another
process, replaces an existing destination or retries other failures. The
blocking actor has not been identified; this is no universal Windows latency
or clean-machine guarantee.

Progress counts engine members only, independently of inventory.json's ZIP
position. After extraction, settings shows **Finishing installation** and keeps
the same progress element indeterminate until verification/promotion completes.
Optional status `failureDetails` retains only a fixed operation token and native
error number; it contains no paths, document text or native error messages.

Current Windows fresh-ZIP settings and the full default archive-to-reader native
flows pass, including cancel/remount recovery, stable finalization controls,
completion while settings is closed, Korean pages, WebP, bookmarks and
source-change rejection. Both fresh trees were fully rehashed after execution.
This supersedes the earlier local fresh-install failure qualification; failed
attempts and the rejected one-minute candidate remain recorded separately.
Linux's shared installer/service regression and actual Office pages also pass.
