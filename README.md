# Prism

Windows-first desktop search workspace built with Tauri 2, Rust, React and TypeScript.

## Current milestone

- Native main window and Spotlight-style quick-search window, with a tray menu and configurable global shortcut.
- Search, chat and document tabs; persistent settings, independent filename/content scopes, and recent query state shared between windows.
- Korean, English, Simplified Chinese, Traditional Chinese and Japanese UI, light/dark/system themes, and adjustable text size.
- A local Markdown draft editor with autosave and Markdown export.
- Real filename/path search over registered folders, extension and modification-date filters, bounded results, live filesystem updates, and open/reveal/copy-path actions.
- CortexJS calculator, math.js unit conversions, and explicit web-search commands in both windows. Empty quick search stays compact without a result or empty-state panel.

Content search, RAG, AI-generated answers, dictionaries and clipboard history are not connected yet. The filename index is an in-memory, portable directory traversal implementation rebuilt at startup. It is not the planned NTFS MFT/USN backend, a persistent catalog, or an Everything-level performance claim. Symlinks and junctions below a registered root are not followed. Only names, paths and filesystem metadata are collected; document contents are not read.

## Search and quick tools

Plain input searches filenames and paths. Space-separated terms are combined with AND; double quotes keep a phrase together. Search is case-insensitive and normalizes Unicode for matching while preserving distinct filesystem paths. A selected folder limits the scope. The main window lists indexed items even with empty input; the quick window shows only its input until a non-whitespace value is entered.

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

File results support ↑/↓ selection, Enter or double-click to open, and toolbar actions to reveal the file or copy its path. Only current indexed paths can be opened. Up to 200 results are displayed with the total match count. The index updates from native filesystem events and reconciles every two minutes to repair missed events; unavailable folders and unreadable entries are reported. Refresh preserves the existing catalog while rescanning, and removing a search scope immediately removes its results.

`edit-date:` accepts `=`, `>`, `>=`, `<` and `<=` with a local-calendar year, month or date (`YYYY`, `YYYY-MM`, `YYYY-MM-DD`). `>2023` starts at 2024-01-01; `>2024-01-01` starts on the next day. The UI extension filter is combined with query filters using AND. Invalid quotes, extensions and dates show an error.

The calculator supports numeric arithmetic, parentheses, powers, percent, factorials up to 1000, `sqrt`, `abs`, trig/inverse trig, `log`/`ln` (natural), `log10`, `exp`, `floor`, `ceil` and `round`. Default display precision is 12 significant digits and trig uses radians; settings support 6–15 digits and degrees. Exact forms such as `1/3` are shown alongside approximations. Collections, assignments, properties and unlisted functions are rejected. A lazily loaded worker is reused when idle, cancelled on changed input, and terminated after two seconds of computation (cold module loading has a separate ten-second limit).

Prism gives implicit multiplication priority over explicit multiplication/division: `48/2(9+3)` is interpreted as `48 / (2 * (9 + 3))` and returns **2**. `48/2*(9+3)` and `(48/2)(9+3)` return **288**. Parentheses override this convention. The interpreted expression is displayed when implicit multiplication is present. This is an explicit product convention; math.js has its own documented implicit-multiplication exceptions, which Prism normalizes before evaluation. See [math.js expression syntax](https://mathjs.org/docs/expressions/syntax.html#implicit-multiplication).

Units cover length, area, mass, volume, temperature, pressure, speed, fuel economy, data and time. Common Korean aliases are accepted. `GB/MB` are decimal and `GiB/MiB` binary; `mpg`, gallon and cup use US measures. Conversion between incompatible categories or values below absolute zero is rejected. Negative quantities are supported for temperature only.

Web input stays local until the user executes a search. It then opens a built-in HTTPS provider with an encoded query. `web:` defaults to Google; `wiki:` currently uses English Wikipedia. Clipboard access is limited to explicit copy actions; history collection is not active.

## Development

Requires Node.js 22.12+ and the [Tauri Windows prerequisites](https://v2.tauri.app/start/prerequisites/), including Rust, MSVC C++ Build Tools and WebView2.

```sh
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

After building this standalone debug executable, run `npm run test:standalone` to verify the embedded UI, settings and window handoff without Vite.

## Native smoke tests

With the Vite server running in another terminal:

```sh
cargo build --manifest-path src-tauri/Cargo.toml
npm run test:native
```

This tests actual WebView2 windows, IPC, shortcut registration, isolated settings persistence, restart recovery and Markdown writes. It uses a temporary data directory through the development-only `PRISM_DEV_DATA_DIR` override, plus a separate WebView2 profile. Isolated debug tests skip the single-instance handoff so they can coexist with an open user app. Normal debug and release apps retain single-instance behavior. Screenshots and test output are saved in the OS temporary directory, outside committed source. The test does not send physical OS keyboard input or exercise native file-picker dialogs.

Planning notes and local verification artifacts are kept in the ignored `.dev/` directory.
