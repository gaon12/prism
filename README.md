# Prism

Windows-first desktop search workspace built with Tauri 2, Rust, React and TypeScript.

## Current milestone

- Native main window and Spotlight-style quick-search window, with a tray menu and configurable global shortcut.
- Search, chat and document tabs; persistent settings, independent filename/content scopes, and recent query state shared between windows.
- Korean, English, Simplified Chinese, Traditional Chinese and Japanese UI, light/dark/system themes, and adjustable text size.
- A local Markdown draft editor with autosave and Markdown export.
- Real filename/path search over registered folders, extension and modification-date filters, bounded results, live filesystem updates, and open/reveal/copy-path actions.
- CortexJS calculator, math.js unit conversions, explicit web-search commands and persistent clipboard history in both windows. Empty quick search stays compact without a result or empty-state panel.
- Scoped local content search, source locations and live document updates, plus real EmbeddingGemma 2 / LanceDB natural-language search with explicit model installation and startup.
- Four-language dictionary lookup and transactional offline packs, with attributed definitions and separate online-cache/offline states.

RAG chat and AI document generation are still being integrated. The filename index is an in-memory directory traversal implementation rebuilt at startup, not the planned NTFS MFT/USN backend or an Everything-level performance claim. Filename-only scopes collect names, paths and metadata; document contents are read only within separately enabled content/AI scopes. Symlinks and junctions below a registered root are not followed.

Content search stores source fingerprints, sections and bounded chunks in local SQLite, with trigram FTS acceleration and explicit partial-result diagnostics. Text/Markdown/code, CSV/TSV, HTML, PDF, DOCX, HWP/HWPX, PPTX, XLSX and ODT/ODS/ODP have extraction readers. Tables retain headers, row keys and cell locations. Cached spreadsheet formulas are not recalculated. Scanned textless PDFs report OCR required; password-based decryption and OCR are not implemented yet. Extraction has bounded file/archive/text budgets and reports skipped documents rather than claiming complete coverage.

Natural-language search uses the actual `google/embeddinggemma-2` model and native LanceDB cosine retrieval. Model/runtime imports are hash-verified. The current packaged runtime supports Windows x64 CPU; model download and startup are explicit actions under AI/model settings. It does not generate chat answers. File changes trigger reconciliation, current sources are rechecked before results/file actions, and unavailable configured folders remain cached without exposing unavailable text. Internal application caches are excluded from content ingestion.

## Search and quick tools

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

## Development

Requires Node.js 22.12+, Rust 1.94+, and the [Tauri Windows prerequisites](https://v2.tauri.app/start/prerequisites/), including Rust, MSVC C++ Build Tools and WebView2.

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

```sh
npm run build
npm test
cargo test --manifest-path src-tauri/Cargo.toml
npm run test:e2e
npm run format:check
```

## Build

```sh
npm run tauri build
```

The Windows executable is produced in `src-tauri/target/release/`; the installer is under `src-tauri/target/release/bundle/nsis/`.

For a standalone development executable with embedded UI:

```sh
npm run tauri build -- --debug --no-bundle
```

Run `src-tauri/target/debug/prism.exe`. Development builds do not register login startup. A normal desktop build needs WebView2 on Windows.

After building this standalone debug executable, run `npm run test:standalone`, `npm run test:clipboard` and `npm run test:dictionary` to verify embedded UI, settings and window handoff without Vite. `npm run test:content` additionally requires the pinned model at `.dev/models/embeddinggemma-2-Q8_0.gguf` and runtime archive at `.dev/runtime/llama-b11960-bin-win-cpu-x64.zip`; it imports them into an isolated temporary profile and performs actual embedding/Lance retrieval.

## Native smoke tests

With the Vite server running in another terminal:

```sh
cargo build --manifest-path src-tauri/Cargo.toml
npm run test:native
```

This tests actual WebView2 windows, IPC, shortcut registration, isolated settings persistence, restart recovery and Markdown writes. It uses a temporary data directory through the development-only `PRISM_DEV_DATA_DIR` override, plus a separate WebView2 profile. Isolated debug tests skip the single-instance handoff so they can coexist with an open user app. Normal debug and release apps retain single-instance behavior. Screenshots and test output are saved in the OS temporary directory, outside committed source. The tests do not send physical OS keyboard input or exercise native file-picker dialogs. Isolated debug profiles inject an in-memory clipboard, including copy commands, so they never read or write the user’s system clipboard. Clipboard fixtures cover text/link/image restoration, filtering, pinning, retention settings, first-run acknowledgment and restart; real Windows clipboard capture remains a separate manual integration check.

Planning notes and local verification artifacts are kept in the ignored `.dev/` directory.
