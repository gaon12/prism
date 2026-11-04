# Prism

Windows-first desktop search workspace built with Tauri 2, Rust, React and TypeScript.

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
```

## Build

```sh
npm run tauri build
```

The Windows executable is produced in `src-tauri/target/release/`; the installer is under `src-tauri/target/release/bundle/nsis/`.

Planning notes and local verification artifacts are kept in the ignored `.dev/` directory.
