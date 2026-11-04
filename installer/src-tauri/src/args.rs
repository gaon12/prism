//! Reads the command line the setup program was started with.
//!
//! People start the setup by double-clicking it (no arguments). Two other callers pass flags:
//! - The Tauri updater inside Prism. It treats every Windows `.exe` update as an NSIS installer
//!   and starts it with NSIS-style flags, e.g. `/P /UPDATE /R /ARGS <Prism's own arguments>`.
//!   Supporting the same flags lets our own installer replace NSIS without changing the updater.
//! - "Apps & features" in Windows Settings, which runs `uninstall.exe` (optionally with `/S`).
//!
//! Supported flags (case-insensitive):
//! | flag              | meaning                                                            |
//! | ----------------- | ------------------------------------------------------------------ |
//! | `/S`              | silent: no window at all, keep the current options                  |
//! | `/P`              | passive: show progress only, no questions                           |
//! | `/UPDATE`         | this is an update of an existing installation                       |
//! | `/R`              | start Prism again after installing                                  |
//! | `/ARGS a b c`     | arguments for the restarted Prism (everything after `/ARGS`)        |
//! | `/D=C:\path`      | install folder (NSIS spelling)                                      |
//! | `--uninstall`     | remove Prism (also implied when this file is named `uninstall.exe`) |
//! | `--from-temp DIR` | internal: uninstaller re-started from %TEMP% to remove `DIR`        |
//! | `--restore-version VERSION` | explicitly restore the matching retained version          |

use std::path::PathBuf;

/// How much user interface the setup shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiMode {
    /// Normal window with options and buttons.
    Full,
    /// Window that only shows progress and closes by itself (`/P`).
    Passive,
    /// No window (`/S`).
    Silent,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetupArgs {
    pub ui: UiMode,
    pub uninstall: bool,
    pub update: bool,
    pub relaunch: bool,
    pub relaunch_args: Vec<String>,
    pub install_dir: Option<PathBuf>,
    /// Set when the uninstaller copied itself to %TEMP% to be able to delete the install folder.
    pub from_temp: Option<PathBuf>,
    /// Explicit target is required for a silent previous-version restore.
    pub restore_version: Option<String>,
}

impl SetupArgs {
    /// `args` are the arguments without the program path; `exe_name` is this file's name, used to
    /// recognise `uninstall.exe`.
    pub fn parse(args: &[String], exe_name: &str) -> Self {
        let mut parsed = Self {
            ui: UiMode::Full,
            uninstall: exe_name.eq_ignore_ascii_case("uninstall.exe"),
            update: false,
            relaunch: false,
            relaunch_args: Vec::new(),
            install_dir: None,
            from_temp: None,
            restore_version: None,
        };
        let mut index = 0;
        while index < args.len() {
            let arg = &args[index];
            let upper = arg.to_ascii_uppercase();
            match upper.as_str() {
                "/S" => parsed.ui = UiMode::Silent,
                // Silent wins if both are given.
                "/P" if parsed.ui != UiMode::Silent => parsed.ui = UiMode::Passive,
                "/P" => {}
                "/UPDATE" => parsed.update = true,
                "/R" => parsed.relaunch = true,
                "/ARGS" => {
                    // Everything after /ARGS belongs to Prism, even values that look like flags.
                    parsed.relaunch_args = args[index + 1..].to_vec();
                    break;
                }
                "--UNINSTALL" => parsed.uninstall = true,
                "--RESTORE-VERSION" => {
                    // Missing/empty is retained as an invalid request, never an install.
                    parsed.restore_version = Some(String::new());
                    if let Some(version) = args
                        .get(index + 1)
                        .filter(|value| !value.starts_with(['-', '/']))
                    {
                        index += 1;
                        parsed.restore_version = Some(version.clone());
                    }
                }
                "--FROM-TEMP" => {
                    index += 1;
                    parsed.from_temp = args.get(index).map(PathBuf::from);
                    parsed.uninstall = true;
                }
                _ if upper.starts_with("/D=") => {
                    let path = arg[3..].trim_matches('"');
                    if !path.is_empty() {
                        parsed.install_dir = Some(PathBuf::from(path));
                    }
                }
                // Unknown flags are ignored so future updater versions cannot break setup.
                _ => {}
            }
            index += 1;
        }
        parsed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(line: &[&str], exe: &str) -> SetupArgs {
        let args: Vec<String> = line.iter().map(|s| s.to_string()).collect();
        SetupArgs::parse(&args, exe)
    }

    #[test]
    fn double_click_is_a_normal_install() {
        let args = parse(&[], "Prism_0.2.0_x64-setup.exe");
        assert_eq!(args.ui, UiMode::Full);
        assert!(!args.uninstall && !args.update && !args.relaunch);
    }

    #[test]
    fn tauri_updater_flags_are_understood() {
        // Exactly what the updater sends in its default "passive" mode with restart enabled.
        let args = parse(
            &["/P", "/UPDATE", "/R", "/ARGS", "--minimized", "/S"],
            "setup.exe",
        );
        assert_eq!(args.ui, UiMode::Passive);
        assert!(args.update && args.relaunch);
        // "/S" after /ARGS belongs to Prism, not to the setup.
        assert_eq!(args.relaunch_args, ["--minimized", "/S"]);
        let quiet = parse(&["/s", "/update", "/r"], "setup.exe");
        assert_eq!(quiet.ui, UiMode::Silent);
        assert_eq!(parse(&["/S", "/P"], "setup.exe").ui, UiMode::Silent);
    }

    #[test]
    fn install_folder_and_uninstall_modes() {
        let args = parse(&[r#"/D=C:\Tools\Prism App"#], "setup.exe");
        assert_eq!(args.install_dir, Some(PathBuf::from(r"C:\Tools\Prism App")));
        assert_eq!(parse(&["/D="], "setup.exe").install_dir, None);
        assert!(parse(&[], "Uninstall.exe").uninstall);
        assert!(parse(&["--uninstall"], "setup.exe").uninstall);
        let temp = parse(&["--from-temp", r"C:\Users\a\AppData\Local\Prism"], "x.exe");
        assert!(temp.uninstall);
        assert_eq!(
            temp.from_temp,
            Some(PathBuf::from(r"C:\Users\a\AppData\Local\Prism"))
        );
        assert_eq!(
            parse(&["--future-flag", "/X"], "setup.exe").ui,
            UiMode::Full
        );
    }

    #[test]
    fn restore_target_is_explicit_and_does_not_consume_app_arguments() {
        let args = parse(&["--restore-version", "0.1.0", "/S"], "uninstall.exe");
        assert_eq!(args.restore_version.as_deref(), Some("0.1.0"));
        assert!(args.uninstall);
        assert_eq!(
            parse(&["--restore-version"], "setup.exe")
                .restore_version
                .as_deref(),
            Some("")
        );
        let missing = parse(&["--restore-version", "/S"], "setup.exe");
        assert_eq!(missing.ui, UiMode::Silent);
        assert_eq!(missing.restore_version.as_deref(), Some(""));
        assert_eq!(
            parse(&["/ARGS", "--restore-version", "0.1.0"], "setup.exe").restore_version,
            None
        );
    }
}
