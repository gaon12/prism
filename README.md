# Prism

Windows-first desktop search workspace built with Tauri 2, Rust, React and TypeScript.

## Current milestone

- Native main window and Spotlight-style quick-search window, with a tray menu and configurable global shortcut.
- First-run language/theme, shortcut, folder and privacy choices, with deferred saving and preserved legacy profiles.
- Search, chat, document and collection tabs; persistent settings, independent filename/content scopes, and recent query state shared between windows.
- IDE-style main workbench with docked tools, a collapsible library, central tabs and a background indexing panel. Live progress bars show actual processing without inventing a final percentage while file traversal is still discovering work.
- Korean, English, Simplified Chinese, Traditional Chinese and Japanese UI, light/dark/system themes, and adjustable text size.
- A local Markdown draft editor with autosave and Markdown export.
- Real filename/path search over registered folders, extension and modification-date filters, bounded results, live filesystem updates, and open/reveal/copy-path actions.
- CortexJS calculator, math.js unit conversions, explicit web-search commands and persistent clipboard history in both windows. Empty quick search stays compact without a result or empty-state panel.
- Scoped local content search, source locations and live document updates, plus real EmbeddingGemma 2 / LanceDB natural-language search with explicit model installation and startup.
- Four-language dictionary lookup and transactional offline packs, with attributed definitions and separate online-cache/offline states.
- Persistent RAG conversations with explicit local/external provider settings, current source previews, cancellation and Markdown export.
- Source-based document drafts with reviewed outlines, manual and selected-text editing, stable citations, immutable revision history, and Markdown/PDF export.
- Reference-only collections with works, volumes/chapters and pages, actual first-page WebP previews, page ordering and a resumable reader.

Collections retain original file locations and never move or delete originals when removing references. Image files, image folders and ZIP/CBZ archives support naturally ordered pages and WebP previews. EPUB uses the package's declared spine order, local author styles/images and XHTML/SVG documents; its actual first viewport becomes the WebP thumbnail, independently of any declared cover artwork. Fixed pages support page/width fitting, while reflowable sections scroll within the reader. Reading bookmarks identify the original spine section. Windows also renders actual PDF pages without requiring OCR to be enabled. A macOS Quartz adapter implements the same preview path, with actual Mac execution still awaiting verification. Linux uses a bundled CPU PDF renderer in a bounded, app-owned subprocess, without requiring OCR, a model download, Python or a system PDF tool. Actual Linux library/process checks cover original page pixels and WebP collection media; Linux desktop/AppImage verification remains pending. Text and supported Office documents currently show extracted text rather than original page layouts; original-page Office/text thumbnails remain pending. Adding a collection does not enable content indexing, AI access or external transmission for its files.

Linux PDF rendering uses [Hayro](https://github.com/LaurenzV/hayro), embedded document resources and bundled standard PDF fonts. Its [third-party notices](src-tauri/notices/pdf-renderer.txt) ship as an application resource. Missing custom fonts and unsupported PDF features can differ from other viewers; full PDF conformance is not claimed. The one-shot worker has bounded input/output, a 2 GiB address-space ceiling, CPU limits and a 20-second caller deadline. Cancellation kills and reaps its own child; a [Linux parent-death signal](https://man7.org/linux/man-pages/man2/PR_SET_PDEATHSIG.2const.html) also handles abrupt app exit. These controls are not a filesystem or network sandbox.

EPUB resources are read in bounded memory without extracting source files. The reader disables scripts, forms and remote resources; cached WebP admission and reads recheck the current source. Unknown content encryption is reported as protected. IDPF/Adobe font-obfuscation handling is implemented and tested with metadata/key-boundary byte fixtures; actual embedded-font rendering needs a separate native check. This is not a complete EPUB reading-system conformance claim: cross-document hyperlinks, navigation-document UI, multimedia, scripted content and fine-grained CFI bookmarks are not implemented. The [independent fixtures](tests/fixtures/epub/README.md) describe the verification scope.

The filename index uses an in-memory catalog, rebuilt at startup. Its folder walker uses up to eight workers and coalesces overlapping roots and repeated watcher paths, so thousands of writes to a downloading model do not by themselves trigger another drive-wide scan.

The first complete sweep is tracked separately from later filesystem updates. Background tasks show **Collecting filenames** until that sweep finishes, then **Updating filenames** during live changes. Both retain an indeterminate progress bar because the total file count is unknown during traversal. Refreshing or changing the scopes starts a new first sweep; errors and skipped entries are reported separately. Search/chat freshness checks still apply while updates run.

On Windows, complete drive-root reconciliations prefer an already running [NTFS service](indexer/README.md). Actual installation, guarded replacement and recovery after a deliberately failed service start have been verified. The installed service has a persistent metadata cache with USN change replay. Newer per-drive/deferred-cache service changes exist in source; their upgrade and installed-service timing remain pending.

The client prepares a large first catalog in private worker trees while validated record batches arrive, using a bounded queue instead of retaining a second whole snapshot of raw rows. Publication waits for the complete service response and open-root validation under the current scope generation. Small snapshots and later sweeps keep the existing entry-reuse path. Only the NTFS protocol dependencies are additionally optimized in development builds; application debug checks remain enabled.

Alternating local bundled debug runs over about 2.05 million C: entries took 10.3/11.1 seconds before this streaming change and 9.2/10.6 seconds afterward, from process launch to complete initial snapshot. An earlier candidate run took 12.4 seconds. Live files and cache conditions vary, so these observations establish neither cold-cache performance nor the requested 5–7 seconds. A separate synthetic two-million-entry comparison retained equal complete metadata while sampled private-memory peaks decreased from 1.15 GB to 1.06 GB in the benchmark process. Total application memory, including WebView processes, was not measured; large catalogs still have substantial memory costs.

The current-source optimized Windows release completed a later 2.055-million-entry C: snapshot in 9.9 seconds, so the 5–7-second target remains unmet. A previous optimized build took 18.3 seconds on its first measured launch and 8.5/8.3 seconds in subsequent alternating runs. These used fresh portable profiles and the existing installed service, without concurrent compilers or cache resets; they do not establish controlled cold-cache performance or a direct Everything comparison. The first slower result is retained.

Search-scope settings show the service's real state and explicit install/start/remove controls, plus **Use bundled version** to replace it with the version included in the current app. Replacement briefly interrupts collection, including other Prism copies using the service. A matching administrator helper is compiled into the app, including portable copies; app startup remains unprivileged and never installs or starts the service automatically. Location/ownership preflight runs before UAC, and isolated test profiles cannot mutate services. Full interrupted-upgrade recovery and installer lifecycle integration remain pending.

**Reset search data.** Under Settings → Search folders → Reset indexes, rebuild the filename catalog independently or reset content and semantic indexes. Filename rebuild keeps the app open and does not delete the shared NTFS service cache. Content reset explicitly restarts Prism, clears only its extracted-text/vector cache before reopening it, and rebuilds enabled content folders. Original files, settings, search state, chats, documents, collections, clipboard history and model assets are kept. Save or discard setting changes first; unsaved documents block the restart action. OCR-derived text is cleared too, so image text needs OCR enabled to be extracted again. Reset does not enable folders or download/start a model.

`npm run test:index-reset` checks this flow with an explicit Windows `PRISM_TEST_EXECUTABLE`, copied into a private portable fixture before launch. It asserts a debug runtime by default; for a genuine release candidate, also set `PRISM_TEST_EXPECT_RELEASE=1`. It performs an actual restart and checks retained records, reindexing, cancelled confirmations and unsaved-change guards. A separate Rust lifecycle test resets actual SQLite and two Lance namespaces after their owning process exits. Native macOS/Linux GUI and release-profile reset results are tracked separately.

Filename-only scopes collect names, paths and metadata; document contents are read only within separately enabled content/AI scopes. Symlinks and junctions below a registered root are not followed. Content extraction, search and RAG share the same path policy. On macOS, the exact OS-managed `/var`, `/tmp` and `/etc` aliases are accepted after checking ownership, protected parents and their expected `/private` targets; user symlinks remain rejected. See Apple's [filesystem definitions](https://github.com/apple-oss-distributions/files/blob/main/Makefile). These aliases and rejection of nested user links pass [native Mac library tests](https://github.com/gaon12/prism/actions/runs/42177612271/job/127759596900); full Mac desktop integration remains unverified.

Content search stores source fingerprints, sections and bounded chunks in local SQLite, with trigram FTS acceleration and explicit partial-result diagnostics. Text/Markdown/code, CSV/TSV, HTML, PDF, DOCX, HWP/HWPX, PPTX, XLSX and ODT/ODS/ODP have extraction readers. Tables retain headers, row keys and cell locations. Cached spreadsheet formulas are not recalculated. Extraction has bounded file/archive/text budgets and reports skipped documents rather than claiming complete coverage.

Under **Search-folder settings → Image and scanned-document OCR**, opt in to local image/scanned-page recognition and choose **Automatic**, **OS built-in OCR** or **PaddleOCR-VL-1.6**. Automatic prefers the OS engine and uses installed Paddle when the OS engine or selected language is unavailable. Windows supports its built-in engine and installed language packs. PNG/JPEG/BMP/TIFF/WebP images and textless PDF pages can be recognized; typed PDF pages keep their text and original page order. Missing engines/languages, unreadable text and processing limits have separate diagnostics. OCR defaults off; switching it off stops new recognition while retaining previously indexed text. Current content/AI scope permissions and external-AI permissions still apply separately.

The macOS adapter uses Vision text recognition and Quartz PDF rasterization with bounded in-memory inputs. It checks the languages actually supported by the OS, applies image orientation and white transparency compositing, and respects PDF crop boxes and page rotation. Cancellation returns promptly while unfinished OS work keeps its concurrency slot until it exits. Intel and Apple Silicon SDK binding checks pass; actual Vision recognition, Quartz output and Mac desktop integration remain unverified. The [platform checks](tools/platform-qa/README.md) describe explicit native tests.

The full PaddleOCR-VL-1.6 pipeline includes both layout analysis and text recognition. Its Windows x64 and Linux x64 CPU packages install a private, hash-verified Python runtime and dependencies without requiring user-installed Python, pip or Docker. Installation is explicit (about 2.2–2.4 GiB download and 3.0–3.5 GiB installed, depending on the platform) and does not enable OCR, expand folder permissions or resume paused indexing. Documents stay in the owned local worker; cancellation terminates its process, and idle workers stop after two minutes. The Linux production runtime has been exercised under WSL with actual Korean image/PDF recognition, automatic fallback, cancellation and app-parent SIGKILL cleanup. Native desktop integration and macOS/ARM64 Paddle packages remain pending. See the [runtime inventory and verification workflow](tools/ocr-runtime/README.md).

For an encrypted PDF, DOCX, XLSX or PPTX, expand the skipped-document list in Content search and choose **Enter password**. Prism does not save the password, change the encrypted original or write a decrypted document. Extracted text is saved in the authorized local content index and can be reused after restart without re-entering the password; a changed original may require it again. Manual retries preserve a paused background index and never start the embedding model. Cancelled or stale requests cannot publish text, and current folder permissions, cache exclusions and original encrypted-byte fingerprints are checked again before publication. PDF user and owner passwords are supported for the tested RC4-40/128 and AES-128/256 formats.

Office password-to-open support covers tested Agile SHA-512/SHA-1 and Office 2007 Standard files, including a Unicode PPTX password. Agile package integrity is required; Standard defines no package HMAC. Wrong passwords, corruption, unsupported encryption, resource limits and timeouts have distinct outcomes. Decryption uses a pinned local Rust codec with bounded memory/KDF work and no installed Office or Python requirement. These tests establish text extraction, not original Office page rendering. Legacy binary Office, HWP encryption, certificate-only protection and other untested schemes remain unsupported. The codec's licenses and derivative notices are bundled in [office-passwords.txt](src-tauri/notices/office-passwords.txt).

Natural-language search uses the actual `google/embeddinggemma-2` model and native LanceDB cosine retrieval. Model/runtime imports are hash-verified. The pinned CPU runtime inventory covers Windows x64, macOS Apple Silicon/Intel, and Linux x64/ARM64; settings show the matching official archive. Model download and startup are explicit actions under AI/model settings. It does not generate chat answers. File changes trigger reconciliation, current sources are rechecked before results/file actions, and unavailable configured folders remain cached without exposing unavailable text. Internal application caches are excluded from content ingestion.

All five official archives have passed extraction and installed-file verification. Actual model inference and Korean retrieval were checked on Windows x64 and Linux x64 under WSL. macOS runtime execution and Linux ARM64 execution remain unverified, as does full macOS/Linux desktop integration. Linux binaries require compatible glibc/libstdc++ versions. Unix library aliases are materialized as verified regular copies. Windows binds the server to a private Job Object. Unix uses a pre-GUI supervisor that watches a private lifetime pipe and kills/reaps its own server after the app stops or dies. Linux tests verified normal cleanup and actual SIGKILL of the parent with both inert fixtures and the real EmbeddingGemma 2 server. The supervisor compiled for both Mac architectures; native Mac execution remains pending.

RAG chat connects a user-run OpenAI-compatible local server or an external HTTPS provider; Prism does not download or start a generative model. Defaults are local, with external questions and fallback disabled. External RAG requires both question opt-in and separate permission for each folder's content. Provider changes and fallback cannot expand those permissions. Credentials are bound to the profile, provider slot and API base. Storage uses Windows user-bound DPAPI, the macOS user Keychain, or the Linux login session's Secret Service (for example GNOME Keyring or KWallet). Start/unlock that service and refresh the key status if it is unavailable. Failures preserve existing entries and return an error without saving plaintext. A real disposable GNOME Keyring session passed roundtrip/replacement/deletion tests; Apple Silicon and Intel credential code passed cross-target checks, with actual macOS Keychain behavior still pending.

Each provider attempt rechecks current policy and source hashes. Both history roles retain hidden/uncited provenance, so revoked, changed, deleted or out-of-scope evidence is omitted rather than replayed. Document questions with no evidence do not silently become general chat. Source links and model citations are unverified references; unknown labels remain unlinked, and reported usage is never estimated. Conversations support search, rename/delete, paging, explicit cancellation and Markdown export. Tests exercise the actual native RAG/source/SQLite/DPAPI path with a synthetic loopback completion server; actual external generation is not verified by that fixture.

**External sources through MCP.** In AI settings, register a local MCP server program and arguments, inspect its tools, and choose a search tool and its question input. Prism calls tools marked read-only or explicitly allowed for that server. In chat, **Also search external sources** is off for each new question. Enabling it sends only that question to the chosen tool, without local document excerpts or conversation history. Results appear separately as untrusted `[E#]` sources. Removing or changing the server, or disabling MCP, hides answers that used its results, including derived follow-up answers, from the current view, exports, conversation search and model history. Restoring the exact server configuration makes them available again; this is permission-based hiding, not physical deletion of stored history.

The native Windows MCP check uses a real stdio server and bundled app with a synthetic loopback generation provider. It covers opt-in, question-only input, source display, revocation, restart and cancellation. Supply an explicit candidate through `PRISM_TEST_EXECUTABLE` before `npm run test:mcp`; the test prepares a private portable copy before launch. For a genuine release candidate, also set `PRISM_TEST_EXPECT_RELEASE=1`. This does not verify a third-party server, external API or real-model answer accuracy.

Document creation does not call AI. Choose explicit content scopes, generate and review an outline, then generate the body through the same RAG policy and provider configuration. A concise purpose/topic drives retrieval independently of the larger draft; keyword fallback requires a topic matching indexed text. Markdown templates, tables, numbers, manual blocks, paragraph/Unicode selection rewrites, version restore and optional source footnotes are supported. Model-assigned fact/user-input/proposal/blank kinds do not verify claims; actual manual edits are marked separately. Derived text retains all inherited source permissions and hashes, so a forbidden or changed source blocks reuse of that draft. Malformed, cancelled and stale operations preserve the prior revision. Drafts stay outside automatic content indexing, and existing local Markdown notes remain separate. Independent external research during document generation, original Office editing beyond form filling, remaining Paddle targets, legacy Office/HWP password processing and binary HWP export are not implemented. Native macOS OCR has an adapter awaiting actual Mac verification.

**Fill an existing form.** In the Documents tab, switch to _Fill an existing form_ (or make it the default under Settings → General). Open a DOCX or HWPX form and Prism lists its `{{name}}` placeholders (the same name is filled everywhere) and blanks of three or more underscores. Type values and save: Prism writes a new file and never modifies the original. Unchanged parts of the package, styles and formatting are copied as-is; a placeholder split across differently formatted runs keeps the formatting of its first run. If the form changes on disk after it was opened, saving is refused until it is reloaded, so a value cannot land in a different blank. Values, fields and the output size are limited, and values with control characters are rejected. HWPX output has not yet been opened in Hancom Office for verification; fields inside text that contains markup are skipped.

In a saved document, expand **Export files** and choose **Export PDF**. The selected immutable version, source-footnote option and block-type option determine the output; save unsaved changes first. PDF export uses an embedded Typst engine with bundled Noto CJK, mono and monochrome emoji fonts, without a printer, separate program or font/model download. A4 pages support headings, paragraphs, lists, quotes, code, repeated table headers and footnotes. Markdown images become visible, counted placeholders; they are not embedded or fetched. Raw HTML is printed as text, and only HTTP(S)/mailto links become PDF links. This is a supported Markdown subset, not an Office-layout round trip.

The renderer receives escaped structure from the backend's saved revision and cannot resolve document files, packages, network resources, environment values or system fonts. Export admits one actual compilation at a time, with bounded input/events/tables/pages/output and a 30-second deadline. Cancellation returns promptly; a compiler already running retains its admission until it finishes and cannot publish afterwards. A same-directory temporary file is atomically renamed only on success, preserving an existing chosen file after errors or cancellation. Once the final rename starts, its real result wins over cancellation. Pinned font provenance, hashes and OFL licenses are in [document-fonts](src-tauri/assets/document-fonts/sources.json); the bundled [PDF notice](src-tauri/notices/document-pdf.txt) includes the engine and Markdown-parser licenses.

**Export HWPX** uses the same saved version and source/classification options. A
pure Rust HwpForge adapter writes real typed paragraphs, tables, code and
footnotes, including visible unverified classification labels. Images become
descriptions, raw HTML becomes text, and resource files or URLs are never
resolved. Unsupported codec diagnostics stop export instead of silently
discarding content. The default template cannot be overridden by Markdown
frontmatter. HWPX declares fonts for the reader to resolve; it does not embed the
PDF engine's font files. Pagination and font substitution depend on the reader.

PDF and HWPX share one actual export worker admission and the same cancellable
atomic destination writer. HWPX additionally bounds nested Markdown and repeated
footnote expansion before decoding. The [independent engine checks](tools/hwpx-export-qa/README.md)
compile the production modules on Windows and Linux. Actual Windows app tests
verify package structure, saved source/marker options, cancellation and reindexing
through Prism's separate extractor. Native Hancom visual rendering and full
Markdown/Office fidelity remain unverified. The bundled [HWPX notice](src-tauri/notices/document-hwpx.txt)
contains the exact pinned codec provenance and MIT license.

본 제품은 한컴의 HWP 문서 파일(.hwp) 공개 문서를 참고하여 개발하였습니다.

## Search and quick tools

New search sessions start in **Automatic** mode. It searches filenames first and, only when none match, searches separately permitted content/AI scopes. An already-running local model enables natural-language retrieval; otherwise it uses content keywords and explains why. It never installs or starts a model automatically. Under **AI/model settings**, disable **Use natural-language search in automatic mode** to keep automatic fallback on keywords. Semantic failures also show a notice before keyword fallback; cancellation and invalid input remain errors. Explicit file-filter syntax and paths stay in filename search. Existing saved modes, including legacy filename sessions, are preserved; manual Filename, Content and Natural language modes remain available.

Natural-language result lists combine content keyword and vector ranks by default. In **AI/model settings**, turn off **Combine content keyword results with natural-language search** for cosine ranking alone. Combined results label keyword/semantic provenance and a **fusion rank score**, which is not cosine similarity or a probability. The service rechecks current scopes and source fingerprints after combining results; both branches use the same extension filter, including comma-separated extensions. If a long query exceeds the lexical term budget, semantic results remain available with an explicit notice. Without semantic retrieval, the input-limit notice distinguishes skipped keyword work from a completed search with no matches. Partial-result counts sum both branches and may include duplicate evaluations. Chat and document retrieval have their own Hybrid, Semantic and Keyword setting under AI/model settings. Hybrid is the default and combines each branch’s global top 100 candidates with the same rank fusion. Saved file sets are searched in batches before global fusion, so later pages do not receive artificial rank-one votes. The context chunk budget still applies independently of selected file count. Without a running local embedding model, retrieval uses keywords; an explicit Keyword setting skips query embedding even when a model is running. Long questions beyond the lexical term budget use semantic retrieval alone in hybrid mode. Current source hashes, selected folders/files and external-transmission permissions still apply.

From main search, choose **Chat with results** or the **Chat** tab to create a conversation restricted to the result files. For filename results, Prism saves every matching file in the current index, including matches beyond the 200 displayed items. For content keyword results, it saves every matching file rather than only the 100 preview chunks or the preview candidate budget. Automatic mode uses the same full content set when its fallback is keyword search. Both paths stream a fixed index view into database rows rather than transferring a large file array through the webview. Changed results require a retry; an unfinished scan or unavailable entries are marked as potentially incomplete. The snapshot preserves file membership, while evidence is still checked against current file contents and permissions. Directories and duplicate chunks from the same file are omitted. For natural-language and hybrid search, the complete handoff is every distinct file in the displayed top-ranked results, including Automatic ranked fallbacks. It stays restricted to those shown files; other documents in the folder are not added automatically.

This action does not send a question. Expand the search-result file selection in chat to edit targets, with 50 files per page for filename and keyword snapshots. **Select all files** and **Deselect files** apply to the whole saved set after saving, including other pages. Save edits before paging or asking; an older conversation refresh cannot overwrite a newer selection or reset its page. A background transcript refresh keeps an otherwise ready question usable. Selections survive restart, and deselecting every file blocks document questions instead of expanding to all folders. **Remove file restriction and use folders** explicitly returns to ordinary folder-based retrieval.

File targets restrict retrieval before ranking limits and do not enable content reading in filename-only scopes or permit external sending. Current folder permissions, source hashes and generation settings still apply. Previous user/assistant pairs influenced by a file excluded from the selection are omitted from subsequent model requests. The provider receives relevant excerpts within the selected files, not the entire result set or a list of absolute local paths. In Spotlight, choose **Chat with results** or press **Ctrl+Shift+Enter** (Command+Shift+Enter on macOS) to save and open the conversation directly in the main window. No question is sent automatically. A queued handoff survives late listener registration; unsaved document edits keep the main window on the document until they are saved or discarded. **Ctrl+Enter** continues to open ordinary main search.

On Windows, existing 8.3 folder aliases resolve to the same current source and cache identity as their long spelling. Explicitly saving settings stores safe existing folders with their long paths and preserves folder IDs and content/external permissions. Cache exclusions and file/scope purges use that identity; source access still rejects links/reparse points and requires current scope, chunk identity and SHA checks. A missing child can reuse its existing parent's identity, but an already missing short-only component cannot be expanded by guessing its former long name.

Plain input starts with filenames and paths; use **Filename** to keep it there. Space-separated filename terms are combined with AND; double quotes keep a phrase together. Search is case-insensitive and normalizes Unicode for matching while preserving distinct filesystem paths. A selected folder limits the scope. The main window lists indexed items even with empty input; the quick window shows only its input until a non-whitespace value is entered. Filename index progress stays visible when automatic fallback is shown during catalog scanning.

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

File results support ↑/↓ selection, Enter or double-click to open, and toolbar actions to reveal the file or copy its path. Only current indexed paths can be opened. Up to 200 filename results are displayed with the total match count.

In **Settings → Shortcuts → Open files and folders**, replace Enter with a key such as Ctrl+O or F4. This setting applies to search inputs and filename/content result lists in both windows. It is separate from the global quick-search shortcut; calculator, dictionary and clipboard commands keep Enter. Plain typing, quick-window Ctrl/Command+Enter navigation/chat chords, Alt+F4 and Ctrl/Command+Q/W are reserved. Opening is suppressed during IME composition and key repeats.

The index updates from native filesystem events and reconciles every five minutes while monitoring is healthy (two minutes when folders or watches need recovery) to repair missed events; unavailable folders and unreadable entries are reported. Refresh preserves the existing catalog while rescanning, and removing a search scope immediately removes its results.

`edit-date:` accepts `=`, `>`, `>=`, `<` and `<=` with a local-calendar year, month or date (`YYYY`, `YYYY-MM`, `YYYY-MM-DD`). `>2023` starts at 2024-01-01; `>2024-01-01` starts on the next day. The UI extension filter is combined with query filters using AND. Invalid quotes, extensions and dates show an error.

The calculator supports numeric arithmetic, parentheses, powers, percent, factorials up to 1000, `sqrt`, `abs`, trig/inverse trig, `log`/`ln` (natural), `log10`, `exp`, `floor`, `ceil` and `round`. Default display precision is 12 significant digits and trig uses radians; settings support 6–15 digits and degrees. Exact forms such as `1/3` are shown alongside approximations. Collections, assignments, properties and unlisted functions are rejected. A lazily loaded worker is reused when idle, cancelled on changed input, and terminated after two seconds of computation (cold module loading has a separate ten-second limit).

The worker constructs only the math.js parser, unit, BigNumber, formatting and OperatorNode dependencies using the [official custom-bundling API](https://mathjs.org/docs/custom_bundling.html). CortexJS still evaluates the allowed numeric expressions. This reduces unnecessary initialization without changing the cold-load or execution deadlines.

Prism gives implicit multiplication priority over explicit multiplication/division: `48/2(9+3)` is interpreted as `48 / (2 * (9 + 3))` and returns **2**. `48/2*(9+3)` and `(48/2)(9+3)` return **288**. Parentheses override this convention. The interpreted expression is displayed when implicit multiplication is present. This is an explicit product convention; math.js has its own documented implicit-multiplication exceptions, which Prism normalizes before evaluation. See [math.js expression syntax](https://mathjs.org/docs/expressions/syntax.html#implicit-multiplication).

Approximate answers state the display precision. When formatting changes the numeric result, the UI explicitly says it was rounded to the configured number of significant digits. Exact answers such as `2` and `1/8 = 0.125`, and scientific notation that preserves the value, do not receive a rounding label. The same distinction applies to unit conversions; copying still copies only the numeric result and unit.

Units cover length, area, mass, volume, temperature, pressure, speed, fuel economy, data and time. Common Korean aliases are accepted. `GB/MB` are decimal and `GiB/MiB` binary; `mpg`, gallon and cup use US measures. Conversion between incompatible categories or values below absolute zero is rejected. Negative quantities are supported for temperature only.

Web input stays local until the user executes a search. It then opens a built-in HTTPS provider with an encoded query. `web:` defaults to Google; `wiki:` currently uses English Wikipedia. Clipboard history is enabled by default with a first-run notice and can be disabled or paused. `clip:`, `clipboard:` or `클립보드:` opens text, link and image history. Filter by kind, search normalized text or Korean initials, preview, copy, pin or delete items, and clear unpinned history. The separate Copy action copies an item; automatic paste behavior and platform support are described below.

Dictionary words are sent only on Enter or explicit lookup. Wiktionary/Kaikki definitions currently use English glosses, labeled in the UI. Installed offline packs are separate from temporary online-result cache; network failures may fall back to installed packs, while a missing online entry is not silently replaced. Packs can be downloaded, imported, cancelled and removed. The bundled native tests use generated four-language fixture packs; live Rust Wiktionary connectivity has not passed in this environment.

History is stored locally and is not encrypted. Defaults keep up to 500 unpinned items for 30 days within 256 MiB; pinned items are not automatically removed and may exceed the budget. Retention, image capture and collection can be changed in the clipboard panel. Windows history-exclusion formats are respected. Existing clipboard contents at startup and content copied while paused or disabled are skipped. Platforms without change markers rebaseline on resume and cannot detect an immediate same-content recopy until a different payload is seen.

On Windows, the clipboard panel's paste action, double-click and search-input Enter restore the previous app and request Ctrl+V. A foreground event hook remembers only the latest external window handle/process ID in memory; it collects no window titles or keystrokes. Targets expire after five minutes and are not saved across restarts. Prism waits briefly for shortcut keys to be released, validates the target again, and hides its windows for the paste. A successful result means the input batch was sent, not that the receiving application acknowledged it.

The separate Copy action stays available. Missing/closed targets, focus restrictions, held keys, blocked/partial input and unsupported platforms produce an explicit result while preserving a successful clipboard copy. Partial input never retries the paste chord; only missing synthetic key releases receive bounded cleanup. Windows foreground and integrity restrictions still apply; Prism does not simulate Alt to bypass them. On macOS, Prism reads the front app with `lsappinfo` and pastes through System Events (`osascript`), which needs the Accessibility permission for Prism; without it the copy is kept and the paste reports blocked input. On Linux, automatic paste works in X11 sessions with `xdotool` installed; Wayland-only sessions keep copy with an automatic-paste-unavailable message. macOS and Linux paste have not yet been verified on real desktops.

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

The [independent platform checks](tools/platform-qa/README.md) compile the exact runtime, credential, extraction, text repository and path modules without Tauri/Lance and document opt-in real archive, model, and OS-store tests. `npm run test:platform-ui` verifies the bundled Windows settings UI with synthetic platform responses. `npm run test:ocr-platform-ui` checks Mac OCR labels, language-tag aliases and capability refresh using the same isolated Windows WebView with synthetic capabilities. Neither establishes macOS/Linux native behavior.

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

`npm run test:release` builds the small [artifact verifier](tools/update-verifier/README.md) and tests version handling, preflight, checksums, manifest generation and portable archive contents. Its signing check creates disposable keys, runs the actual Tauri signer on inert fixture bytes, verifies signatures and versions, and removes the temporary key files without printing them. It uses no production keys, certificate trust changes, installer execution or release publishing. `PRISM_UPDATE_VERIFIER` can select a separately built verifier when using a custom Cargo target directory.

Workflow validation covers actionlint, Bash syntax and local helper integration. The Windows unsigned overlay and actual standalone app have been run locally. The [hosted three-OS build](https://github.com/gaon12/prism/actions/runs/42177612271) produced and verified artifacts at commit `7ef747b`; newer changes and explicit native Mac gates need their own results. Windows Authenticode, Apple notarization and a complete signed updater installation in an ordinary user profile remain unverified. See the [Tauri updater guide](https://v2.tauri.app/plugin/updater/) for artifact formats and signing, and [GitHub workflow syntax](https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax) for job permissions and manual inputs.

### Windows portable package

After building the Windows x64 executable, run `npm run package:portable`. It packages `src-tauri/target/release/prism.exe` into a ZIP and SHA-256 checksum under `src-tauri/target/release/bundle/portable/`. To package a standalone debug build for local review:

```sh
npm run package:portable -- --exe src-tauri/target/debug/prism.exe --out .dev/build/portable
```

Portable and MSIX packaging require the four component notices beside the selected executable in `notices/` and preserve their bytes in the package. Missing or empty notices fail packaging before output is created. `npm run test:package-portable` checks archive entries, notice bytes, checksums and declared-resource coverage using an inert executable fixture. The MSIX SDK test verifies the same notices after unpacking.

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

`npm run test:pdf-unlock` uses independently generated encrypted PDF fixtures with an isolated profile. It checks wrong passwords, cleared inputs, cancellation during the real prepare handshake, stale tokens, filename-only folder rejection, unchanged originals, page numbering and password-free cache reopening. It then embeds unlocked text with the same actual pinned Q8/runtime assets and searches the resulting Lance vectors. Generation is not used. Set `PRISM_TEST_EXECUTABLE` to test a standalone executable built in another target directory.

`npm run test:office-unlock` uses five independently sourced/encrypted DOCX/XLSX/PPTX fixtures and the actual bundled app. It checks Unicode and wrong passwords, cleared inputs, window-bound/cancelled tokens, filename-only folder rejection, strict PDF command compatibility, paused retries, original encrypted fingerprints, actual keyword results and cached text after restart. Originals remain unchanged, decrypted document files and persisted passwords are absent, and no embedding model starts. Korean desktop and English dark narrow-window screenshots cover the dialog. The [independent engine checks](tools/office-unlock-qa/README.md) separately compare complete decrypted packages on Windows/Linux; neither check proves native Word/Excel/PowerPoint layout or Mac/Linux desktop integration.

`npm run test:ocr` requires the Windows Korean OCR language pack and the same real pinned Q8/runtime assets. Independent pixel-only PNG and PDF fixtures exercise actual native OCR, transparent backgrounds, original mixed-page order, an AES-256 password retry, off/retention/restart policy, missing-language UI and scope revocation. The test observes a real indeterminate progress bar during processing, then retrieves OCR text from actual LanceDB. Recognition never downloads a language pack or substitutes a fake engine/model.

`npm run test:indexing-ui` adds a real watched file and holds one actual search response to verify the chat action row remains mounted and disabled until its fresh selection arrives. The same test failed against the previous executable because indexing unmounted the row. Browser workbench tests also cover desktop/mobile panels, keyboard dismissal, themes and large text.

`npm run test:rag` uses generated files, an isolated profile, synthetic credentials and a loopback generation fixture. It checks current evidence, permission/credential boundaries, persisted conversations, cancellation and the actual chat UI without a real provider key or external request.

`npm run test:rag-hybrid` uses the actual pinned EmbeddingGemma 2 Q8 model, FTS and LanceDB with 266 synthetic documents. It compares chat evidence against whole-scope keyword, semantic and hybrid ranks while the saved set crosses a 256-file batch boundary, and exercises all three settings through the native UI. Only generation uses a synthetic loopback provider. It requires the pinned model and Windows runtime archives at the same local fixture paths as the content test.

`npm run test:search-chat` checks the actual main-window handoff, saved target editing, tainted-history exclusion, empty-selection IPC enforcement and restart persistence. A 1,006-file query passes every match despite displaying only 200; the test retrieves evidence from a file beyond that display limit, edits the last file page, rejects a stale revision and applies bulk selection across pages. A latency fixture holds an actual transcript read response without changing its contents to verify that it cannot disable a ready question. Generation uses a synthetic loopback server, and the documents and app profile are generated fixtures. It also checks 390px layout; this is Windows native UI evidence, not macOS/Linux desktop verification.

Set `PRISM_TEST_SEARCH_CHAT_MODEL=1` when running that command to exercise the selected-set path with the actual pinned EmbeddingGemma 2 assets described above. The ordinary run uses keyword retrieval without installing a model. The model run imports local verified assets and starts them explicitly; it does not call an external AI provider.

`npm run test:chat-entry` exercises full keyword handoff with 107 indexed files behind a 100-chunk preview, then asks for evidence from the last file. It checks Spotlight filename and keyword entry, button and Ctrl+Shift+Enter actions, main-only handoff consumption, an empty compact window, responsive controls and deferred navigation while a document has unsaved input. It uses the actual app, index and SQLite with synthetic documents and loopback generation; no embedding model is needed.

`npm run test:short-paths` uses actual Win32 8.3 scope/profile paths to check indexing, private-cache exclusions, RAG, canonical settings persistence and folder disconnect/reconnect. It uses synthetic documents and loopback generation. Non-Windows systems and volumes without short aliases report a skip; set `PRISM_REQUIRE_SHORT_PATH=1` to require alias coverage on Windows.

`npm run test:documents` uses the same isolated native services and a synthetic loopback generator. It verifies explicit outline review, structured blocks, manual edits, Unicode selection rewrites, long drafts, invalid/cancelled response preservation, source changes, immutable restore/export/restart and actual document controls. It does not verify a real model's structured-output quality or operate the OS save dialog.

`npm run test:document-pdf` uses a private native profile and the real saved-revision/PDF/file/collection-rendering services. It checks old versus current versions, foreign-version rejection, dirty-edit export controls, early cancellation, multilingual text, page geometry and actual first-page WebP pixels. Only the save-picker response is supplied by the test; it also temporarily holds a real export IPC to exercise cancellation overtaking admission. `test:documents` additionally checks actual saved RAG-source footnotes and classification options in PDF output without another provider call. These two developer tests require Python with `pypdf` for independent PDF inspection (`PRISM_TEST_PYTHON` can select the interpreter); the application does not require Python for PDF export. They do not prove actual native save-dialog interaction or Mac/Linux desktop integration.

The document fixture delays actual body/status/manual-save operations without replacing their data. A late active-status read cannot undo a locally completed generation; manual-save assertions wait for the persisted revision rather than an already-present local edit marker.

`npm run test:document-hwpx` uses actual saved revisions, the HWPX encoder, atomic
file replacement and a private content index. It checks old/current/foreign
versions, main-window restrictions, dirty edits, shared PDF controls, early
cancellation preserving an existing destination, literal code/HTML, no resource
fetches and independent multilingual extraction without starting embeddings.
Only the save-picker response is supplied and one real export IPC is held before
admission. Its independent ZIP/XML reader needs Python's standard library only;
Prism itself requires no Python or installed Hancom for HWPX export.
`test:documents` checks both PDF and HWPX source/marker options against saved RAG
citations without another provider call. These tests do not verify native
Hancom layout or actual OS save-dialog interaction.

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
