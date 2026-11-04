# Prism

Windows-first desktop search workspace built with Tauri 2, Rust, React and TypeScript.

## Current milestone

- Native main window and Spotlight-style quick-search window, with a tray menu and configurable global shortcut.
- First-run language/theme, shortcut, folder and privacy choices, with deferred saving and preserved legacy profiles.
- Search, chat and document tabs; persistent settings, independent filename/content scopes, and recent query state shared between windows.
- Korean, English, Simplified Chinese, Traditional Chinese and Japanese UI, light/dark/system themes, and adjustable text size.
- A local Markdown draft editor with autosave and Markdown export.
- Real filename/path search over registered folders, extension and modification-date filters, bounded results, live filesystem updates, and open/reveal/copy-path actions.
- CortexJS calculator, math.js unit conversions, explicit web-search commands and persistent clipboard history in both windows. Empty quick search stays compact without a result or empty-state panel.
- Scoped local content search, source locations and live document updates, plus real EmbeddingGemma 2 / LanceDB natural-language search with explicit model installation and startup.
- Four-language dictionary lookup and transactional offline packs, with attributed definitions and separate online-cache/offline states.
- Persistent RAG conversations with explicit local/external provider settings, current source previews, cancellation and Markdown export.
- Source-based Markdown document drafts with reviewed outlines, manual and selected-text editing, stable citations and immutable revision history.

The filename index is an in-memory directory traversal implementation rebuilt at startup, not the planned NTFS MFT/USN backend or an Everything-level performance claim. Filename-only scopes collect names, paths and metadata; document contents are read only within separately enabled content/AI scopes. Symlinks and junctions below a registered root are not followed.

Content search stores source fingerprints, sections and bounded chunks in local SQLite, with trigram FTS acceleration and explicit partial-result diagnostics. Text/Markdown/code, CSV/TSV, HTML, PDF, DOCX, HWP/HWPX, PPTX, XLSX and ODT/ODS/ODP have extraction readers. Tables retain headers, row keys and cell locations. Cached spreadsheet formulas are not recalculated. Scanned textless PDFs report OCR required; password-based decryption and OCR are not implemented yet. Extraction has bounded file/archive/text budgets and reports skipped documents rather than claiming complete coverage.

Natural-language search uses the actual `google/embeddinggemma-2` model and native LanceDB cosine retrieval. Model/runtime imports are hash-verified. The pinned CPU runtime inventory covers Windows x64, macOS Apple Silicon/Intel, and Linux x64/ARM64; settings show the matching official archive. Model download and startup are explicit actions under AI/model settings. It does not generate chat answers. File changes trigger reconciliation, current sources are rechecked before results/file actions, and unavailable configured folders remain cached without exposing unavailable text. Internal application caches are excluded from content ingestion.

All five official archives have passed extraction and installed-file verification. Actual model inference and Korean retrieval were checked on Windows x64 and Linux x64 under WSL. macOS runtime execution and Linux ARM64 execution remain unverified, as does full macOS/Linux desktop integration. Linux binaries require compatible glibc/libstdc++ versions. Unix library aliases are materialized as verified regular copies. Windows binds the server to a private Job Object. Unix uses a pre-GUI supervisor that watches a private lifetime pipe and kills/reaps its own server after the app stops or dies. Linux tests verified normal cleanup and actual SIGKILL of the parent with both inert fixtures and the real EmbeddingGemma 2 server. The supervisor compiled for both Mac architectures; native Mac execution remains pending.

RAG chat connects a user-run OpenAI-compatible local server or an external HTTPS provider; Prism does not download or start a generative model. Defaults are local, with external questions and fallback disabled. External RAG requires both question opt-in and separate permission for each folder's content. Provider changes and fallback cannot expand those permissions. Credentials are bound to the profile, provider slot and API base. Storage uses Windows user-bound DPAPI, the macOS user Keychain, or the Linux login session's Secret Service (for example GNOME Keyring or KWallet). Start/unlock that service and refresh the key status if it is unavailable. Failures preserve existing entries and return an error without saving plaintext. A real disposable GNOME Keyring session passed roundtrip/replacement/deletion tests; Apple Silicon and Intel credential code passed cross-target checks, with actual macOS Keychain behavior still pending.

Each provider attempt rechecks current policy and source hashes. Both history roles retain hidden/uncited provenance, so revoked, changed, deleted or out-of-scope evidence is omitted rather than replayed. Document questions with no evidence do not silently become general chat. Source links and model citations are unverified references; unknown labels remain unlinked, and reported usage is never estimated. Conversations support search, rename/delete, paging, explicit cancellation and Markdown export. Tests exercise the actual native RAG/source/SQLite/DPAPI path with a synthetic loopback completion server; actual external generation is not verified by that fixture.

Document creation does not call AI. Choose explicit content scopes, generate and review an outline, then generate the body through the same RAG policy and provider configuration. A concise purpose/topic drives retrieval independently of the larger draft; keyword fallback requires a topic matching indexed text. Markdown templates, tables, numbers, manual blocks, paragraph/Unicode selection rewrites, version restore and optional source footnotes are supported. Model-assigned fact/user-input/proposal/blank kinds do not verify claims; actual manual edits are marked separately. Derived text retains all inherited source permissions and hashes, so a forbidden or changed source blocks reuse of that draft. Malformed, cancelled and stale operations preserve the prior revision. Drafts stay outside automatic content indexing, and existing local Markdown notes remain separate. External research, original Office editing, OCR/password processing and PDF/HWP/HWPX export are not implemented.

## Search and quick tools

Natural-language result lists combine content keyword and vector ranks by default. In **AI/model settings**, turn off **Combine content keyword results with natural-language search** for cosine ranking alone. Combined results label keyword/semantic provenance and a **fusion rank score**, which is not cosine similarity or a probability. The service rechecks current scopes and source fingerprints after combining results; both branches use the same extension filter. If a long query exceeds the lexical term budget, semantic results remain available with an explicit notice. Partial-result counts sum both branches and may include duplicate evaluations. Automatic mode selection and use of hybrid retrieval in RAG remain pending.

Plain input searches filenames and paths. Space-separated terms are combined with AND; double quotes keep a phrase together. Search is case-insensitive and normalizes Unicode for matching while preserving distinct filesystem paths. A selected folder limits the scope. The main window lists indexed items even with empty input; the quick window shows only its input until a non-whitespace value is entered.

Korean initials work alongside literal terms and filters: `ㄱㅁㅅ ext:hwpx` can find `공문서.hwpx`. Mixed input such as `ㄱ문서` keeps `문서` literal rather than matching every word with initials `ㄱㅁㅅ`. Doubled consonants remain distinct, and both compatibility jamo (`ㄱ`) and modern leading jamo (`ᄀ`) are supported. Literal filename matches rank ahead of initial matches.

If the current scope and filters find no literal or initial matches, Prism can retry using Korean two-set/QWERTY physical keys: `rhdanstj` can find `공문서`, and Korean keys accidentally used for `prism` can find `Prism-guide.txt`. Corrected results are labeled and the entered query stays unchanged. General settings can disable this fallback. This is keyboard-layout correction, not general spelling correction; URLs, drive paths, filter syntax and single Latin letters are not converted.

| Input                                     | Action                                                             |
| ----------------------------------------- | ------------------------------------------------------------------ |
| `공문서 계획`                             | Names/paths containing both terms                                  |
| `extension:hwpx edit-date:>2023 공문서`   | HWPX files modified in 2024 or later, with 공문서 in the name/path |
| `ext:pdf,md`                              | PDF or Markdown files                                              |
| `= 2+3*4` or `calc: sqrt(2)`              | Calculate locally; Enter copies the answer                         |
| `unit: 10인치 to cm`                      | Convert units locally                                              |
| `unit: 20 km/L to L/100km`                | Convert reciprocal fuel economy                                    |
| `web: 검색어` or `google: 검색어`         | Search Google in the default browser                               |
| `bing:`, `ddg:`, `namu:`, `wiki:` + query | Search Bing, DuckDuckGo, NamuWiki or Wikipedia                     |
| `dict:ko 사과`, `dict:ja りんご`          | Manually look up Korean, English, Chinese or Japanese words        |
| `clip:`                                   | Search local text, link and image clipboard history                |

File results support ↑/↓ selection, Enter or double-click to open, and toolbar actions to reveal the file or copy its path. Only current indexed paths can be opened. Up to 200 results are displayed with the total match count. The index updates from native filesystem events and reconciles every five minutes while monitoring is healthy (two minutes when folders or watches need recovery) to repair missed events; unavailable folders and unreadable entries are reported. Refresh preserves the existing catalog while rescanning, and removing a search scope immediately removes its results.

`edit-date:` accepts `=`, `>`, `>=`, `<` and `<=` with a local-calendar year, month or date (`YYYY`, `YYYY-MM`, `YYYY-MM-DD`). `>2023` starts at 2024-01-01; `>2024-01-01` starts on the next day. The UI extension filter is combined with query filters using AND. Invalid quotes, extensions and dates show an error.

The calculator supports numeric arithmetic, parentheses, powers, percent, factorials up to 1000, `sqrt`, `abs`, trig/inverse trig, `log`/`ln` (natural), `log10`, `exp`, `floor`, `ceil` and `round`. Default display precision is 12 significant digits and trig uses radians; settings support 6–15 digits and degrees. Exact forms such as `1/3` are shown alongside approximations. Collections, assignments, properties and unlisted functions are rejected. A lazily loaded worker is reused when idle, cancelled on changed input, and terminated after two seconds of computation (cold module loading has a separate ten-second limit).

Prism gives implicit multiplication priority over explicit multiplication/division: `48/2(9+3)` is interpreted as `48 / (2 * (9 + 3))` and returns **2**. `48/2*(9+3)` and `(48/2)(9+3)` return **288**. Parentheses override this convention. The interpreted expression is displayed when implicit multiplication is present. This is an explicit product convention; math.js has its own documented implicit-multiplication exceptions, which Prism normalizes before evaluation. See [math.js expression syntax](https://mathjs.org/docs/expressions/syntax.html#implicit-multiplication).

Approximate answers state the display precision. When formatting changes the numeric result, the UI explicitly says it was rounded to the configured number of significant digits. Exact answers such as `2` and `1/8 = 0.125`, and scientific notation that preserves the value, do not receive a rounding label. The same distinction applies to unit conversions; copying still copies only the numeric result and unit.

Units cover length, area, mass, volume, temperature, pressure, speed, fuel economy, data and time. Common Korean aliases are accepted. `GB/MB` are decimal and `GiB/MiB` binary; `mpg`, gallon and cup use US measures. Conversion between incompatible categories or values below absolute zero is rejected. Negative quantities are supported for temperature only.

Web input stays local until the user executes a search. It then opens a built-in HTTPS provider with an encoded query. `web:` defaults to Google; `wiki:` currently uses English Wikipedia. Clipboard history is enabled by default with a first-run notice and can be disabled or paused. `clip:`, `clipboard:` or `클립보드:` opens text, link and image history. Filter by kind, search normalized text or Korean initials, preview, copy, pin or delete items, and clear unpinned history. Enter copies the selected item; automatic paste into the previous app is not implemented yet.

Dictionary words are sent only on Enter or explicit lookup. Wiktionary/Kaikki definitions currently use English glosses, labeled in the UI. Installed offline packs are separate from temporary online-result cache; network failures may fall back to installed packs, while a missing online entry is not silently replaced. Packs can be downloaded, imported, cancelled and removed. The bundled native tests use generated four-language fixture packs; live Rust Wiktionary connectivity has not passed in this environment.

History is stored locally and is not encrypted. Defaults keep up to 500 unpinned items for 30 days within 256 MiB; pinned items are not automatically removed and may exceed the budget. Retention, image capture and collection can be changed in the clipboard panel. Windows history-exclusion formats are respected. Existing clipboard contents at startup and content copied while paused or disabled are skipped. Platforms without change markers rebaseline on resume and cannot detect an immediate same-content recopy until a different payload is seen.

On Windows, the clipboard panel's paste action, double-click and search-input Enter restore the previous app and request Ctrl+V. A foreground event hook remembers only the latest external window handle/process ID in memory; it collects no window titles or keystrokes. Targets expire after five minutes and are not saved across restarts. Prism waits briefly for shortcut keys to be released, validates the target again, and hides its windows for the paste. A successful result means the input batch was sent, not that the receiving application acknowledged it.

The separate Copy action stays available. Missing/closed targets, focus restrictions, held keys, blocked/partial input and unsupported platforms produce an explicit result while preserving a successful clipboard copy. Partial input never retries the paste chord; only missing synthetic key releases receive bounded cleanup. Windows foreground and integrity restrictions still apply; Prism does not simulate Alt to bypass them. macOS/Linux currently retain copy with an automatic-paste-unavailable message.

## Updates

Normal desktop profiles check the public GitHub release manifest 30 seconds after startup and then at most once a day. General settings can disable automatic checks or request a manual check. Dismissal is remembered per version; failed automatic checks stay quiet, while a manual check reports connection failures. Requests carry a generic Prism version User-Agent and no document contents, questions or user identifiers. Browser preview does not check or invent releases.

Installation requires an explicit confirmation. The package download is bounded, and Tauri verifies both the signature and its bound version before Prism saves settings, cancels AI generation and pauses search work. Failed preparation/installation restores search scopes, clipboard pause state and a previously running search model; cancelled AI generation is not replayed. Unsaved document/settings changes block restart. Corrupt settings are never replaced with defaults during update preparation.

Portable copies, development builds and release builds without an update verification key offer the download page. For installed release builds, supply the public key content through `PRISM_UPDATER_PUBLIC_KEY` when compiling. Keep the signing private key outside the repository. Updater signatures are required independently of Windows Authenticode. Custom Windows update artifacts must be signed with `tauri signer sign --app-version <version>`; `requireSignedVersion` rejects older unbound signatures and a manifest that pairs a new version with an older signed artifact. See the [official Tauri updater guide](https://v2.tauri.app/plugin/updater/).

MSIX copies use their Windows distribution source for updates. They never launch the ordinary installer updater or offer its download button. New-version notices remain available, with guidance to Microsoft Store or the source used to install the package.

The current local preview has no production verification key and does not self-install updates. The Windows setup recovers interrupted transactions and can restore a retained previous app version. Both have process-termination and sandbox UI verification. Previous-version restoration keeps current settings, documents and indexes, requires confirmation, and leaves app launch unchecked. Signed updater-initiated installation against a normal installed profile, power-loss recovery, real older-release data compatibility and release publication remain integration work.

## Reporting a problem

Open **Settings → General → Report a problem** in the desktop app and choose **Preview diagnostics**. The snapshot includes the app version, OS and architecture, distribution type, initialization/error flags and aggregate counts. It excludes document text, paths, queries, clipboard contents, API keys, generation endpoints and raw error messages. Unavailable status fields are `null`; an initialized service is not a guarantee that every operation works.

Add reproduction steps, then review the complete report before copying it or saving it as Markdown. Notes you type are included, so check them for personal information. Refreshing diagnostics preserves your notes. Nothing is submitted automatically: **Report on GitHub** opens a fixed issue page without report data in its URL. GitHub issues are public and require an account; paste and submit the report yourself. This is an on-demand report, not a background crash-upload service.

`npm run test:diagnostics` exercises real native snapshot collection, privacy sentinels, report preview, MemoryClipboard copying, Markdown writing, cancelled save and retry behavior in an isolated debug profile. File-picker selection and the external-browser handoff are intercepted; the actual OS picker and GitHub submission are separate manual checks.

## Development

Requires Node.js 22.12+, Rust 1.94+, and the [Tauri Windows prerequisites](https://v2.tauri.app/start/prerequisites/), including Rust, MSVC C++ Build Tools and WebView2.

Desktop builds link an `rlib` into the executable to keep intermediate disk usage manageable.

`npm run version:check` checks app and setup versions across eight configuration, manifest and lockfiles. Use `npm run version:bump -- patch` (or `minor`, `major`, an explicit SemVer) to update them together without changing dependency versions or file formatting. The command validates every file before writing and attempts to restore earlier writes if a write fails; it is not a crash-safe multi-file transaction. Commit the version files together. `npm run test:version` verifies the command against temporary fixtures without changing this checkout's version.

`npm run test:setup-core` checks the [setup core](installer/core/README.md), including deletion boundaries, failed-install recovery, concurrency and recovery after externally terminating owned fixture processes. Registration and files stay in temporary fixtures; real Windows COM shortcuts also stay there. `npm run test:setup` verifies the actual setup UI, silent update, installed app launch and removal inside a sandbox. `npm run test:setup-recovery` verifies the real setup UI against an interrupted transaction with a damaged live install record, preserving user files without launching Prism. See the [Windows setup instructions](installer/README.md) for build and executable overrides. Normal-profile registration, signed updater installation and power-loss behavior remain separate checks.

`npm run test:setup-previous` checks the actual previous-version confirmation, stale-target rejection, restored bytes, user-file/data preservation and silent restoration through an installed uninstaller's temporary copy. Version labels and the old app bytes are synthetic; no old app is launched and no real HKCU registration is changed. Restoring files does not prove that an older released app can read a newer data schema.

The native LanceDB build also requires `protoc`. Install a compatible [official Protocol Buffers release](https://github.com/protocolbuffers/protobuf/releases) and put its executable on PATH, or set `PROTOC` for the build shell. The Windows vector tests were verified with protoc 34.1.

```powershell
$env:PROTOC = "C:\tools\protobuf\bin\protoc.exe"
npm ci
npm run tauri dev
```

For UI development in a browser:

```sh
npm run dev
```

Browser preview does not have native filesystem, global-shortcut or tray access.

## Checks

The [independent platform checks](tools/platform-qa/README.md) compile the exact runtime and credential modules without Tauri/Lance and document opt-in real archive, model, and OS-store tests. `npm run test:platform-ui` verifies the bundled Windows settings UI with synthetic platform responses; it does not establish macOS/Linux native behavior.

```sh
npm run build
npm test
npm run test:release
cargo test --manifest-path tools/update-verifier/Cargo.toml --locked
cargo test --manifest-path src-tauri/Cargo.toml
npm run test:e2e
npm run format:check
```

## Build

```sh
npm run tauri -- build --no-bundle
npm run build:setup
```

The Windows executable is produced in `src-tauri/target/release/`; Prism's own setup executable is `installer/src-tauri/target/release/prism-setup.exe`. The default app build does not select an installer format. Portable and MSIX packaging use the commands below.

On macOS, explicitly select `--target universal-apple-darwin --bundles app,dmg` after installing both Apple Rust targets. On Linux, select `--bundles appimage`. These builds also need the platform's [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/). Actual macOS/Linux compilation and native behavior remain CI/integration checks.

For a standalone development executable with embedded UI:

```sh
npm run tauri build -- --debug --no-bundle
```

Run `src-tauri/target/debug/prism.exe`. Development builds do not register login startup. A normal desktop build needs WebView2 on Windows.

### Release workflow

The [release workflow](.github/workflows/release.yml) builds Windows setup/portable, universal macOS app/DMG and Linux AppImage downloads. Manual runs build artifacts by default. A matching `v<version>` tag, or a manual run on that tag with `create_draft` selected, can create a draft after all jobs succeed. The workflow never publishes or replaces a release. Unsigned MSIX files stay in a separate `inspection-windows-msix` artifact while signing/distribution is undecided.

For signed updater builds, configure repository variable `PRISM_UPDATER_PUBLIC_KEY` with the public key content and secret `TAURI_SIGNING_PRIVATE_KEY` with the corresponding private key. An optional `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` secret supplies its password. Keep private material outside source control. Configure both keys or neither; incomplete settings fail preflight. Without keys, the build disables self-install updates and does not generate `latest.json`. Windows Authenticode, MSIX certificate trust and Apple notarization are separate from updater signing.

`scripts/release-config.mjs` checks all version files and the selected tag, then writes a public-only Tauri overlay with explicit platform bundles. Windows custom setup signing includes `--app-version`. Every updater artifact is checked against the same public key embedded in the app; a different key or signed version fails the job. The draft job rechecks exact download names and SHA-256 sidecars before generating the complete verified manifest. Prerelease Windows builds additionally require repository variable `PRISM_MSIX_PACKAGE_VERSION` to assign a noncolliding four-part package version.

`npm run test:release` builds the small [artifact verifier](tools/update-verifier/README.md) and tests version handling, preflight, checksums and manifest generation. Its final test creates disposable keys, runs the actual Tauri signer on inert fixture bytes, verifies signatures and versions, and removes the temporary key files without printing them. It uses no production keys, certificate trust changes, installer execution or release publishing. `PRISM_UPDATE_VERIFIER` can select a separately built verifier when using a custom Cargo target directory.

Workflow validation covers actionlint, Bash syntax and local helper integration. The Windows unsigned overlay and actual standalone app have been run locally. GitHub-hosted macOS/Linux builds, production signing, notarization and signed updater installation have not been run yet; a green local helper test does not establish those platform results. See the [Tauri updater guide](https://v2.tauri.app/plugin/updater/) for artifact formats and signing, and [GitHub workflow syntax](https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax) for job permissions and manual inputs.

### Windows portable package

After building the Windows x64 executable, run `npm run package:portable`. It packages `src-tauri/target/release/prism.exe` into a ZIP and SHA-256 checksum under `src-tauri/target/release/bundle/portable/`. To package a standalone debug build for local review:

```sh
npm run package:portable -- --exe src-tauri/target/debug/prism.exe --out .dev/build/portable
```

Extract the ZIP into a writable folder and keep `prism.portable` beside `prism.exe`. Settings, clipboard history, documents, conversations, model assets, indexes and both windows' WebView2 storage live in `PrismData/` beside the executable. Moving that folder preserves the managed data; registered source folders still use their configured paths. Saved API keys are bound to the Windows account and must be entered again on a different account. The WebView2 Runtime is required separately.

A blocked data directory stops startup with an error instead of falling back to an installed profile. Portable mode disables login startup, excludes its managed data from content indexing and rejects a `WEBVIEW2_USER_DATA_FOLDER` override. Separate portable copies have independent single-instance identities. To update manually, close Prism, replace the application files and preserve `PrismData/`. Packaging does not sign or publish the application, include downloaded model assets or install WebView2.

### Windows MSIX inspection package

With the Windows SDK installed, run `npm run package:msix` after building the x64 release executable. For a local debug inspection artifact:

```sh
npm run package:msix -- --exe src-tauri/target/debug/prism.exe --out .dev/build/msix
npm run test:msix
```

The script uses MakeAppx validation, private staging and exclusive output creation. It writes the package, SHA-256 checksum and identity metadata; it preserves preexisting output files. Stable app versions map to four-part package versions. Prerelease/build versions require an explicit `--package-version`; `--store` additionally enforces Store version rules. The current `0.1.0` version is not a Store release version. Publisher and identity must match the eventual signing/distribution identity.

Unsigned output is for inspection. Normal installation requires a properly signed package or Store distribution; enabling Developer Mode alone does not make an ordinary unsigned package installable. Optional `--sign-thumbprint` selects an existing certificate in the current user's My store and verifies the signed result. Packaging never creates or trusts a certificate, changes Windows settings or registers a package. See Microsoft's [MSIX signing overview](https://learn.microsoft.com/en-us/windows/msix/package/signing-package-overview) and [Store version rules](https://learn.microsoft.com/en-us/windows/apps/publish/publish-your-app/package-version-numbering).

The app queries its actual Windows package family, keeps settings, assets and WebView data under the package's `LocalState`, and uses a separate single-instance identity. Package data is excluded from content indexing. Releases use the manifest `PrismStartup` task instead of an absolute registry autostart path; Windows user/policy restrictions produce localized guidance. Debug builds do not change login startup. See Microsoft's [desktop startup-task documentation](https://learn.microsoft.com/en-us/windows/apps/desktop/modernize/desktop-to-uwp-extensions).

Verification covers real SDK pack/unpack, ordinary-process identity, data-mode policy and synthetic startup-task states. Native update QA also checks the package-specific presentation using a synthetic event. Actual MSIX installation, activation, Windows-managed updates and login startup remain unverified; these tests do not claim that Windows registered a package.

After building this standalone debug executable, run `npm run test:standalone`, `npm run test:clipboard` and `npm run test:dictionary` to verify embedded UI, settings and window handoff without Vite. `npm run test:content` additionally requires the pinned model at `.dev/models/embeddinggemma-2-Q8_0.gguf` and runtime archive at `.dev/runtime/llama-b11960-bin-win-cpu-x64.zip`; it imports them into an isolated temporary profile and performs actual embedding/Lance retrieval.

`npm run test:rag` uses generated files, an isolated profile, synthetic credentials and a loopback generation fixture. It checks current evidence, permission/credential boundaries, persisted conversations, cancellation and the actual chat UI without a real provider key or external request.

`npm run test:documents` uses the same isolated native services and a synthetic loopback generator. It verifies explicit outline review, structured blocks, manual edits, Unicode selection rewrites, long drafts, invalid/cancelled response preservation, source changes, immutable restore/export/restart and actual document controls. It does not verify a real model's structured-output quality or operate the OS save dialog.

`npm run test:onboarding` verifies actual bundled first-run completion, shortcut/settings persistence, clipboard opt-out, skip without applying draft choices and restart. Recommended user folders are explicitly unchecked, and the isolated profile uses MemoryClipboard. Existing profiles missing the new completion marker bypass setup; fresh profiles see the tour. Theme/language choices are saved only on completion.

`npm run test:clipboard-ui` checks retention input validation, filtered/global unpinned counts, pinned-only clear controls and keyboard selection scrolling in both windows using 40 generated entries and MemoryClipboard. Clipboard retention fields accept whole item/day/MiB values; invalid input stays editable and cannot be submitted.

`npm run test:paste` exercises real clipboard services, IPC and both bundled windows with a synthetic foreground/input backend. It verifies copy-preserving failures, concurrent request rejection, text/link/image restoration, paste/copy buttons, row double-click, Spotlight Enter, window recovery and restart expiry. Rust tests cover partial key batches and bounded release cleanup. Isolated profiles never install the foreground hook or send physical keys. Real Windows hook delivery, focus restrictions and target-application acceptance remain manual integration checks.

`npm run test:updates` uses public synthetic signatures with Tauri's actual verifier to reject changed bytes, wrong keys, version mismatch and unbound signatures. Native settings, release notices, dismissal, offline behavior, bookkeeping exclusion, failed-install recovery and restart persistence use a synthetic transport. A synthetic package event verifies that settings and notification controls do not offer an ordinary installer, even with an inconsistent install flag. It downloads no package and launches no installer; test private keys were deleted after fixture creation. Isolated profiles disable the automatic update worker and use MemoryClipboard.

`npm run test:portable` requires the same pinned model/runtime and standalone debug executable. It packages and extracts the actual executable, checks copy-specific single-instance behavior, and performs real EmbeddingGemma 2/Lance retrieval before and after moving the application to a Unicode path. It also checks persisted settings, notes, documents, conversations, synthetic clipboard data, account-bound credentials and blocked startup paths. Its debug-only test flag injects MemoryClipboard while retaining actual portable path selection; ordinary isolated native tests continue to use `PRISM_DEV_DATA_DIR`. It does not exercise physical paste, installer/update behavior or actual external generation.

## Native smoke tests

With the Vite server running in another terminal:

```sh
cargo build --manifest-path src-tauri/Cargo.toml
npm run test:native
```

This tests actual WebView2 windows, IPC, shortcut registration, isolated settings persistence, restart recovery and Markdown writes. It uses a temporary data directory through the development-only `PRISM_DEV_DATA_DIR` override, plus a separate WebView2 profile. Isolated debug tests skip the single-instance handoff so they can coexist with an open user app. Normal debug and release apps retain single-instance behavior. Screenshots and test output are saved in the OS temporary directory, outside committed source. The tests do not send physical OS keyboard input or exercise native file-picker dialogs. Isolated debug profiles inject an in-memory clipboard, including copy commands, so they never read or write the user’s system clipboard. Clipboard fixtures cover text/link/image restoration, filtering, pinning, retention settings, first-run acknowledgment and restart; real Windows clipboard capture remains a separate manual integration check.

Planning notes and local verification artifacts are kept in the ignored `.dev/` directory.
