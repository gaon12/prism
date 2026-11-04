# Prism setup (Windows)

Prism's own installer, used instead of the NSIS installer Tauri would generate. One small
program installs, updates and uninstalls Prism for the current user (no administrator rights).

```
installer/
  core/        tests over the exact production modules, without the WebView UI
  src-tauri/   Rust: install steps (install.rs), command line (args.rs),
               embedded app files (payload.rs), Windows integration (system.rs)
  ui/          the setup window (plain HTML/CSS/JS, 5 languages in messages.js)
  msix/        MSIX manifest template used by scripts/package-msix.mjs
```

## Build

```sh
npm run tauri build -- --no-bundle          # 1. build Prism (src-tauri/target/release/prism.exe)
npm run build:setup                        # 2. setup with Prism packed inside
```

`build.rs` packs `src-tauri/target/release/prism.exe` (or every file in `PRISM_PAYLOAD_DIR`)
and reads the version from `src-tauri/tauri.conf.json`. Without an app build the setup is
created empty only in development builds and refuses to install. Release builds
fail if the app is missing. Payload collection rejects links and unreadable entries.
The native cleanup worker is compiled and embedded at build time; no shell script
is used to delete temporary uninstall copies.

## Command line

| flag                         | meaning                                                      |
| ---------------------------- | ------------------------------------------------------------ |
| (none)                       | setup window                                                 |
| `/P`                         | progress only (Prism's updater uses this)                    |
| `/S`                         | no window                                                    |
| `/UPDATE`, `/R`, `/ARGS ...` | update, restart Prism afterwards, Prism's arguments          |
| `/D=C:\path`                 | install folder                                               |
| `uninstall.exe [/S]`         | remove Prism (settings are kept unless chosen in the window) |

Everything after `/ARGS` belongs to Prism. Do not append `/D=` through Tauri's
extra installer arguments when restart is enabled: Tauri places those after `/ARGS`.
The installed directory is normally read from the per-user installation entry.
The bootstrap uninstaller starts an owned temporary copy and exits; silent callers
must observe removal completion rather than treating bootstrap exit as completion.

## Testing without touching your PC

Set `PRISM_SETUP_SANDBOX` to an empty folder. Desktop, Start menu and AppData then live inside
that folder and the "Apps & features" entry is written to `uninstall-entry.json` there instead
of the registry. The setup WebView also stays in the sandbox. Empty/relative sandbox
values fail instead of falling back to Windows integration, and explicit install
paths outside the sandbox are rejected. Real `.lnk` files are created only there.
In Git Bash also set `MSYS_NO_PATHCONV=1`, otherwise `/S` is turned into a path.

```sh
MSYS_NO_PATHCONV=1 PRISM_SETUP_SANDBOX='C:\temp\prism-sandbox' prism-setup.exe /S
```

`npm run test:setup-core` runs the core tests. For native QA, build a debug setup
with `PRISM_PAYLOAD_DIR` pointing to a directory containing only the desired
standalone debug `prism.exe`. Then run `npm run test:setup`. It defaults to
`installer/src-tauri/target/debug/prism-setup.exe` and compares the installed bytes
with `src-tauri/target/debug/prism.exe`. Override them with `PRISM_SETUP_EXE` and
`PRISM_SETUP_EXPECTED_EXE` if using a different build output. These tests are for
the isolated debug app, not a normal release profile.

The test creates its own OS-temp sandbox, checks actual HKCU registration is
unchanged, rejects concurrent native/independent setup requests, tests normal and
Unicode install paths, `/R /ARGS`, visible and silent removal, preservation of
user files/settings and native temporary-copy cleanup. Screenshots go to the
OS-temp `prism-ui-evidence` directory. No original user documents are indexed;
the launched debug app uses its own data directory and MemoryClipboard.

The cleanup worker remains as a small verified binary in the per-user OS-temp
`PrismSetupTools` cache. It only removes fixed owned uninstall files and never
recursively deletes a path supplied on its command line. Install-folder locks
also live in an OS-temp cache and are released by the OS if a process exits.

## Interrupted installation recovery

Setup writes a versioned journal before changing installed files. It flushes the
original bytes and records hashes for backups, the file map and registration.
The next install or removal in the same folder recovers an unfinished transaction
under the folder lock, then proceeds. A completed journal only needs cleanup.
The window announces pending recovery before installation; folder and disk-space
checks do not restore files by themselves.

Recovery validates every backup and limits destinations to the selected install
folder and its known Prism shortcuts. Corrupt, ambiguous or old unversioned data
is preserved with an error. Changed shortcuts and registration belonging to a
different installation are also preserved. Unpublished staging without a journal
is left in place. Manual restoration of a previous released version is separate.

`npm run test:setup-core` externally terminates its own temporary fixture processes
at four transaction boundaries. `npm run test:setup-recovery` then provides a
separate real-window check: it creates a process-exit journal, damages the current
install record and verifies recovery, the new installed bytes and preserved user
files. It never launches the embedded app and supports release setup builds too.
The executable overrides are the same as `test:setup`; optional
`PRISM_SETUP_CORE_TARGET` selects the core build directory, or
`PRISM_RECOVERY_FIXTURE` supplies its already-built test executable.

Process termination has been tested; power loss and storage hardware failures
have not. Actual HKCU registration, signed updater installation, MSIX and release
publication still require integration checks.

Preview the window design in a browser: open `ui/index.html?preview=options` (also `progress`,
`uninstall`).
