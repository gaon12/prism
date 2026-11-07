# Prism Linux XDG autostart patch

Upstream: `auto-launch` 0.6.0, copied from the exact crates.io archive selected by
the original app lockfile. Archive SHA-256:
`17918dba7ecf78b9a14507ec9f984e7977d13fad87dc86d61038980a45d128cf`.
The upstream MIT license and original platform implementations are retained.

The genuine release AppImage reproduced the problem with two isolated configuration
directories: even with an absolute `XDG_CONFIG_HOME`, startup reconciliation and
the settings toggle read/wrote/deleted `HOME/.config/autostart/prism.desktop`.
The desktop session uses its configured XDG directory, so the toggle could leave
the active registration unchanged. The release probe replaced only the HOME
configuration view inside a private filesystem namespace; it did not modify the
user's login settings.

`get_xdg_autostart_dir()` now uses `dirs::config_dir()` instead of assembling
`HOME/.config` itself. This existing dependency honors absolute `XDG_CONFIG_HOME`
and falls back to `HOME/.config` for unset, empty or relative values. Enable,
disable and status already share this helper. AppImage executable selection,
desktop-entry contents, systemd mode, Windows and macOS are unchanged. No legacy
entry is migrated or removed from another configuration directory.

The regression runs each environment in a separate child process with a fresh
owned HOME and configuration directories. It checks the four environment cases,
status before/after enable, repeated disable, preservation of another config's
same-named entry and unrelated entries, and the AppImage/background command.

Run only these isolated tests and the pure library tests:

```sh
cargo test --manifest-path src-tauri/vendor/auto-launch/Cargo.toml --locked --test xdg_autostart
cargo test --manifest-path src-tauri/vendor/auto-launch/Cargo.toml --locked --lib
```

The upstream `tests/test.rs` integration suite changes actual OS login settings;
it is not part of the Prism regression invocation. The standalone crate lockfile
is retained from upstream. The app uses its own lockfile and this Cargo patch.
A corrected shipping AppImage still needs its own native release qualification;
passing this dependency regression does not change or qualify the old artifact.

Reference: [Freedesktop Autostart Directories](https://specifications.freedesktop.org/autostart/latest/#autostart-directories).
Remove this patch when a selected upstream release implements the same behavior
and the isolated regression passes without it.
