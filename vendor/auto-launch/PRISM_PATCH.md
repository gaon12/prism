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
systemd mode, Windows and macOS are unchanged. No legacy
entry is migrated or removed from another configuration directory.

The regression runs each environment in a separate child process with a fresh
owned HOME and configuration directories. It checks the four environment cases,
status before/after enable, repeated disable, preservation of another config's
same-named entry and unrelated entries, and the AppImage/background command.

The old release also produced an invalid `Exec` entry when launched from an
actual Korean path containing spaces. The generated entry was rejected by the
real GDesktopAppInfo loader. Arguments and executable paths now use desktop-entry
string escaping and whole-argument quoting, including literal percent signs.

For executable paths containing `%` or `=`, a fixed `/bin/sh -c 'exec "$@"'`
launcher receives the target and arguments as positional data. No target or
argument is inserted into the shell program. This handles GIO's executable check
before percent expansion, and the desktop specification's restriction on `=`
in the first executable token. The shell replaces itself with the target.
Other executable paths launch directly.

The explicitly selected real-GIO check creates only an owned argv recorder and
uses a private DBus session and HOME/XDG directories. It checks seven cases:
Korean/spaced paths, reserved punctuation with and without percent, equals signs,
literal arguments through direct and wrapped execution, and a simple path.
The recorder verifies actual argument bytes and boundaries, including empty
arguments, control-character escapes and strings that must remain unexpanded.

Run the isolated configuration and pure library tests with the app's lockfile:

```sh
cargo test --manifest-path src-tauri/Cargo.toml --locked -p auto-launch --lib --test xdg_autostart
```

On a Linux host with `gio` and `dbus-run-session`, explicitly run the real launcher
check, which is otherwise ignored:

```sh
cargo test --manifest-path src-tauri/Cargo.toml --locked -p auto-launch --test xdg_exec -- real_gio_preserves_executable_and_arguments --ignored --exact
```

The upstream `tests/test.rs` integration suite changes actual OS login settings;
it is not part of the Prism regression invocation. The standalone crate lockfile
is retained from upstream. The app uses its own lockfile and this Cargo patch.
A corrected shipping AppImage still needs its own native release qualification;
passing this dependency regression does not change or qualify the old artifact.

Reference: [Freedesktop Autostart Directories](https://specifications.freedesktop.org/autostart/latest/#autostart-directories).
Command syntax: [Desktop Entry Exec](https://specifications.freedesktop.org/desktop-entry/latest/exec-variables.html)
and [value escapes](https://specifications.freedesktop.org/desktop-entry/latest/value-types.html).
GIO behavior: [GLib 2.88 executable validation and expansion](https://github.com/GNOME/glib/blob/2.88.0/gio/gdesktopappinfo.c).
Remove this patch when a selected upstream release implements the same behavior
and the isolated regression passes without it.
