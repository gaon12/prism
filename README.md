# Prism

Windows-first desktop search workspace built with Tauri 2, Rust, React and TypeScript.

## Current milestone

- Native main window and Spotlight-style quick-search window, with a tray menu and configurable global shortcut.
- Search, chat and document tabs; persistent settings, independent filename/content scopes, and recent query state shared between windows.
- Korean, English, Simplified Chinese, Traditional Chinese and Japanese UI, light/dark/system themes, and adjustable text size.
- A local Markdown draft editor with autosave and Markdown export.

This is the client milestone. Folder registration is functional; filesystem indexing, actual filename results, content search, RAG, AI-generated answers, and Spotlight utility modules are not connected yet. The interface labels these states explicitly.

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

This tests actual WebView2 windows, IPC, shortcut registration, isolated settings persistence, restart recovery and Markdown writes. It uses a temporary data directory through the development-only `PRISM_DEV_DATA_DIR` override. Screenshots and test output are saved in the OS temporary directory, outside committed source. The test does not send physical OS keyboard input or exercise native file-picker dialogs.

Planning notes and local verification artifacts are kept in the ignored `.dev/` directory.
