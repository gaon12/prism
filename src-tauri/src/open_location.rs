//! Opening a search result's original file *at the matched place* (requirement Q30).
//!
//! There is no universal "open at page N / line N" API, so this works with the app the user
//! already chose as the default for that file type:
//! 1. Work out where the match is: a PDF page, or a line in a plain-text file.
//! 2. Find the default app for the file:
//!    - Windows: the file association (`AssocQueryStringW`).
//!    - Linux: the explicit default in the XDG `mimeapps.list` files and that app's `.desktop`
//!      entry. Nothing is guessed when no explicit default is set.
//!    - macOS: the app Launch Services would open the file with (`NSWorkspace`).
//! 3. If that app is one we know accepts a page/line argument, start it with that argument.
//!    Otherwise the caller opens the file normally, exactly as before.
//!
//! Nothing here invents a viewer the user did not choose, and nothing is run through a shell.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Where in the file the match is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Location {
    /// 1-based PDF page.
    Page(u32),
    /// 1-based line in a text file.
    Line(usize),
}

/// One program start: the exact executable and its literal arguments (no shell).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub program: PathBuf,
    pub args: Vec<OsString>,
}

/// File types read as plain text, where a line number is meaningful.
pub fn is_text_extension(extension: &str) -> bool {
    matches!(
        extension.to_ascii_lowercase().as_str(),
        "txt"
            | "md"
            | "markdown"
            | "csv"
            | "tsv"
            | "json"
            | "jsonl"
            | "xml"
            | "yaml"
            | "yml"
            | "toml"
            | "ini"
            | "cfg"
            | "log"
            | "rs"
            | "ts"
            | "tsx"
            | "js"
            | "jsx"
            | "py"
            | "java"
            | "c"
            | "h"
            | "cpp"
            | "hpp"
            | "cs"
            | "go"
            | "rb"
            | "php"
            | "sh"
            | "ps1"
            | "sql"
            | "css"
            | "scss"
            | "kt"
            | "swift"
    )
}

/// The 1-based line where `excerpt` starts inside `file_text`, or None if it cannot be found.
///
/// The index stores trimmed paragraphs, so this searches for the excerpt's first non-empty
/// line (at most 120 characters, whitespace-trimmed) rather than the whole chunk.
pub fn line_of(file_text: &str, excerpt: &str) -> Option<usize> {
    let needle: String = excerpt
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?
        .chars()
        .take(120)
        .collect();
    if needle.is_empty() {
        return None;
    }
    let at = file_text.find(&needle)?;
    Some(file_text[..at].matches('\n').count() + 1)
}

/// Arguments that make `viewer` open `path` at `location`. None when that viewer is unknown
/// or cannot jump to this kind of location.
///
/// `viewer` is the default app's executable file name: `Code.exe` on Windows, `code` or
/// `evince` on Linux (the program from its `.desktop` entry). Only names whose page/line
/// option is documented are listed; everything else falls back to a normal open.
pub fn viewer_args(viewer: &str, path: &Path, location: Location) -> Option<Vec<OsString>> {
    let name = viewer.to_ascii_lowercase();
    let file = path.as_os_str().to_os_string();
    // "file:12" for editors that take the line after a colon.
    let with_line = |line: usize| {
        let mut target = file.clone();
        target.push(format!(":{line}"));
        target
    };
    match location {
        Location::Page(page) => match name.as_str() {
            // Browsers' built-in PDF viewers read the page from the URL fragment.
            "msedge.exe"
            | "chrome.exe"
            | "firefox.exe"
            | "brave.exe"
            | "firefox"
            | "firefox-esr"
            | "google-chrome"
            | "google-chrome-stable"
            | "chromium"
            | "chromium-browser"
            | "microsoft-edge"
            | "microsoft-edge-stable"
            | "brave-browser"
            | "brave-browser-stable" => {
                Some(vec![format!("{}#page={page}", file_url(path)).into()])
            }
            // Adobe Acrobat / Reader: /A "page=N" <file>.
            "acrobat.exe" | "acrord32.exe" => {
                Some(vec!["/A".into(), format!("page={page}").into(), file])
            }
            "sumatrapdf.exe" => Some(vec!["-page".into(), page.to_string().into(), file]),
            // GNOME Evince and its MATE/Cinnamon forks: --page-index is the real page number.
            "evince" | "atril" | "xreader" => {
                Some(vec![format!("--page-index={page}").into(), file])
            }
            "okular" => Some(vec!["--page".into(), page.to_string().into(), file]),
            "zathura" => Some(vec![format!("--page={page}").into(), file]),
            _ => None,
        },
        Location::Line(line) => match name.as_str() {
            "code.exe"
            | "code - insiders.exe"
            | "cursor.exe"
            | "code"
            | "code-insiders"
            | "cursor"
            | "codium" => Some(vec!["--goto".into(), with_line(line)]),
            "notepad++.exe" => Some(vec![format!("-n{line}").into(), file]),
            "sublime_text.exe" | "sublime_text" | "subl" => Some(vec![with_line(line)]),
            "gedit" => Some(vec![format!("+{line}").into(), file]),
            "kate" => Some(vec!["--line".into(), line.to_string().into(), file]),
            _ => None,
        },
    }
}

/// Percent-encodes a path for a URL, keeping only plain URL characters and `/` and `:`,
/// so `#`, `%`, `?`, `&`, spaces and non-ASCII names survive. The result has no leading `/`.
fn encode_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    let text = text
        .strip_prefix(r"\\?\")
        .unwrap_or(&text)
        .replace('\\', "/");
    let mut encoded = String::new();
    for byte in text.trim_start_matches('/').bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/:".contains(&byte) {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// `file:///C:/My%20Docs/a.pdf` (Windows) or `file:///home/me/a%20b.pdf` for a local path.
fn file_url(path: &Path) -> String {
    format!("file:///{}", encode_path(path))
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
/// macOS: how to open `path` at `location` with the default app, identified by its bundle
/// identifier (e.g. `com.microsoft.VSCode`). `app` is the `.app` folder and `executable`
/// its main program. None when the app is unknown or cannot jump to this location.
///
/// Editors with a documented URL scheme get that URL through `/usr/bin/open -b <id>`, which
/// hands it to exactly that app (also when it is already running). Chromium browsers are
/// started directly with a `#page=` URL, as on Windows.
pub fn mac_launch(
    bundle_id: &str,
    app: &Path,
    executable: &Path,
    path: &Path,
    location: Location,
) -> Option<Launch> {
    let id = bundle_id.to_ascii_lowercase();
    let open_url = |url: String| Launch {
        program: PathBuf::from("/usr/bin/open"),
        args: vec!["-b".into(), bundle_id.into(), url.into()],
    };
    match location {
        Location::Page(page) => match id.as_str() {
            "com.google.chrome" | "com.microsoft.edgemac" | "com.brave.browser" => Some(Launch {
                program: executable.to_path_buf(),
                args: vec![format!("{}#page={page}", file_url(path)).into()],
            }),
            _ => None,
        },
        Location::Line(line) => {
            // VS Code family: <scheme>://file/<absolute path>:<line>.
            let scheme = match id.as_str() {
                "com.microsoft.vscode" => Some("vscode"),
                "com.microsoft.vscodeinsiders" => Some("vscode-insiders"),
                "com.todesktop.230313mzl4w4u92" => Some("cursor"),
                _ => None,
            };
            if let Some(scheme) = scheme {
                return Some(open_url(format!(
                    "{scheme}://file/{}:{line}",
                    encode_path(path)
                )));
            }
            match id.as_str() {
                // TextMate 2: txmt://open?url=<file URL>&line=<line>.
                "com.macromates.textmate" => Some(open_url(format!(
                    "txmt://open?url={}&line={line}",
                    file_url(path)
                ))),
                // Sublime Text ships its command-line tool inside the app.
                "com.sublimetext.4" | "com.sublimetext.3" => {
                    let subl = app.join("Contents/SharedSupport/bin/subl");
                    let mut target = path.as_os_str().to_os_string();
                    target.push(format!(":{line}"));
                    subl.is_file().then(|| Launch {
                        program: subl,
                        args: vec![target],
                    })
                }
                _ => None,
            }
        }
    }
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
/// Linux: the MIME type whose default app is looked up. Only exact, well-known types are
/// listed; for anything else the file opens normally (we never pick a "close enough" app).
pub fn linux_mime(extension: &str) -> Option<&'static str> {
    Some(match extension.to_ascii_lowercase().as_str() {
        "pdf" => "application/pdf",
        "txt" => "text/plain",
        "md" | "markdown" => "text/markdown",
        "csv" => "text/csv",
        "json" => "application/json",
        "xml" => "application/xml",
        "log" => "text/x-log",
        _ => return None,
    })
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
/// The XDG folders that hold `mimeapps.list` and `.desktop` files, plus the program search
/// path. Built from the environment in the app; tests pass their own folders.
#[derive(Debug, Clone, Default)]
pub struct XdgDirs {
    pub config_home: Option<PathBuf>,
    pub config_dirs: Vec<PathBuf>,
    pub data_home: Option<PathBuf>,
    pub data_dirs: Vec<PathBuf>,
    /// Lowercase desktop names from `XDG_CURRENT_DESKTOP`, e.g. `gnome`.
    pub desktops: Vec<String>,
    /// Folders from `PATH`, for an `Exec=` program given without a folder.
    pub path: Vec<PathBuf>,
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
impl XdgDirs {
    /// The folders from the environment, with the defaults the XDG spec defines.
    pub fn from_env() -> Self {
        let var = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty());
        let home = var("HOME").map(PathBuf::from);
        let list = |name: &str, default: &str| -> Vec<PathBuf> {
            let value = var(name).unwrap_or_else(|| default.into());
            std::env::split_paths(&value)
                .filter(|dir| dir.is_absolute())
                .collect()
        };
        XdgDirs {
            config_home: var("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .or_else(|| home.as_ref().map(|home| home.join(".config"))),
            config_dirs: list("XDG_CONFIG_DIRS", "/etc/xdg"),
            data_home: var("XDG_DATA_HOME")
                .map(PathBuf::from)
                .or_else(|| home.as_ref().map(|home| home.join(".local/share"))),
            data_dirs: list("XDG_DATA_DIRS", "/usr/local/share:/usr/share"),
            desktops: var("XDG_CURRENT_DESKTOP")
                .map(|value| {
                    value
                        .to_string_lossy()
                        .split(':')
                        .filter(|name| !name.is_empty())
                        .map(str::to_ascii_lowercase)
                        .collect()
                })
                .unwrap_or_default(),
            path: var("PATH")
                .map(|value| std::env::split_paths(&value).collect())
                .unwrap_or_default(),
        }
    }

    /// `mimeapps.list` files in the order the XDG MIME Applications spec checks them:
    /// user config, system config, then the (deprecated) data folders; each folder's
    /// desktop-specific `<desktop>-mimeapps.list` before its plain `mimeapps.list`.
    fn mimeapps_files(&self) -> Vec<PathBuf> {
        let mut folders: Vec<PathBuf> = Vec::new();
        folders.extend(self.config_home.clone());
        folders.extend(self.config_dirs.iter().cloned());
        folders.extend(self.data_home.iter().map(|dir| dir.join("applications")));
        folders.extend(self.data_dirs.iter().map(|dir| dir.join("applications")));
        let mut files = Vec::new();
        for folder in folders {
            for desktop in &self.desktops {
                files.push(folder.join(format!("{desktop}-mimeapps.list")));
            }
            files.push(folder.join("mimeapps.list"));
        }
        files
    }

    /// Where a `.desktop` file with this id can be: `applications/<id>`, or for an id like
    /// `kde-okular.desktop` also `applications/kde/okular.desktop` (spec: "-" may stand for
    /// a subfolder).
    fn desktop_files(&self, id: &str) -> Vec<PathBuf> {
        let mut names = vec![PathBuf::from(id)];
        if let Some((folder, rest)) = id.split_once('-') {
            names.push(Path::new(folder).join(rest));
        }
        let mut files = Vec::new();
        for dir in self.data_home.iter().chain(&self.data_dirs) {
            for name in &names {
                files.push(dir.join("applications").join(name));
            }
        }
        files
    }
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
/// The desktop ids listed for `mime` under `[Default Applications]` in one `mimeapps.list`.
pub fn default_entries(mimeapps: &str, mime: &str) -> Vec<String> {
    let mut in_defaults = false;
    let mut entries = Vec::new();
    for line in mimeapps.lines().map(str::trim) {
        if line.starts_with('[') {
            in_defaults = line == "[Default Applications]";
        } else if in_defaults {
            if let Some((key, value)) = line.split_once('=') {
                if key.trim() == mime {
                    entries.extend(
                        value
                            .split(';')
                            .map(str::trim)
                            .filter(|id| id.ends_with(".desktop") && !id.contains(['/', '\\']))
                            .map(str::to_owned),
                    );
                }
            }
        }
    }
    entries
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
/// The program named first in a `.desktop` file's `Exec=` line (its `[Desktop Entry]`
/// group), with the spec's escapes and double quotes removed. None for hidden entries,
/// non-applications and empty or unparsable lines.
pub fn desktop_program(desktop: &str) -> Option<String> {
    let mut in_entry = false;
    let mut exec = None;
    for line in desktop.lines().map(str::trim) {
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match (key.trim(), value.trim()) {
            ("Hidden", "true") => return None,
            ("Type", kind) if kind != "Application" => return None,
            ("Exec", value) => exec = Some(value.to_owned()),
            _ => {}
        }
    }
    let exec = exec?;
    // Value escapes first (\s = space, \\ = backslash), then the Exec quoting rules.
    let mut value = String::new();
    let mut chars = exec.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            value.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => value.push(' '),
            Some('t') => value.push('\t'),
            Some('n') => value.push('\n'),
            Some('r') => value.push('\r'),
            Some('\\') => value.push('\\'),
            // Keep "\x" so the quoting pass below can read its own escapes.
            Some(other) => {
                value.push('\\');
                value.push(other);
            }
            None => return None,
        }
    }
    let value = value.trim_start();
    let mut program = String::new();
    if let Some(quoted) = value.strip_prefix('"') {
        let mut chars = quoted.chars();
        loop {
            match chars.next()? {
                '"' => break,
                '\\' => program.push(chars.next()?),
                c => program.push(c),
            }
        }
    } else {
        program = value.split_whitespace().next()?.to_owned();
    }
    (!program.is_empty() && !program.contains('%')).then_some(program)
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
/// Linux: the executable of the user's explicit default app for `mime`, or None when no
/// installed default is set (then the file opens normally and the desktop decides).
pub fn linux_default_program(mime: &str, dirs: &XdgDirs) -> Option<PathBuf> {
    // Small, local text files only.
    let read = |path: &Path| -> Option<String> {
        let metadata = std::fs::metadata(path).ok()?;
        if !metadata.is_file() || metadata.len() > 256 * 1024 {
            return None;
        }
        std::fs::read_to_string(path).ok()
    };
    for list in dirs.mimeapps_files() {
        let Some(text) = read(&list) else {
            continue;
        };
        for id in default_entries(&text, mime) {
            // The first listed app that is actually installed wins.
            let program = dirs
                .desktop_files(&id)
                .iter()
                .find_map(|file| read(file))
                .and_then(|desktop| desktop_program(&desktop))
                .and_then(|program| resolve_program(&program, &dirs.path));
            if program.is_some() {
                return program;
            }
        }
    }
    None
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
/// An `Exec=` program as an existing file: absolute as written, or found in `PATH`.
fn resolve_program(program: &str, path: &[PathBuf]) -> Option<PathBuf> {
    let program = Path::new(program);
    if program.is_absolute() {
        return program.is_file().then(|| program.to_path_buf());
    }
    if program.components().count() != 1 {
        return None; // "./x" or "dir/x": relative to an unknown folder.
    }
    path.iter()
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
/// Linux: the launch for `path` at `location` with the default app from `dirs`.
pub fn linux_launch(path: &Path, location: Location, dirs: &XdgDirs) -> Option<Launch> {
    let extension = path.extension()?.to_str()?;
    let program = linux_default_program(linux_mime(extension)?, dirs)?;
    let name = program.file_name()?.to_str()?;
    let args = viewer_args(name, path, location)?;
    Some(Launch { program, args })
}

/// Starts `launch` without a shell. On Unix a small thread waits for the program so it
/// does not linger as a zombie process after it exits.
fn start(launch: Launch) -> std::io::Result<()> {
    let mut command = std::process::Command::new(&launch.program);
    command.args(&launch.args);
    #[cfg(unix)]
    {
        use std::process::Stdio;
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        std::thread::Builder::new()
            .name("prism-open-location".into())
            .spawn(move || {
                let _ = child.wait();
            })?;
    }
    #[cfg(not(unix))]
    command.spawn()?;
    Ok(())
}

/// Starts the default app for `path` at `location` when it supports that. Returns Ok(true)
/// when started, Ok(false) when the caller should fall back to a normal open.
pub fn open(path: &Path, location: Location) -> std::io::Result<bool> {
    let plain = path
        .to_string_lossy()
        .strip_prefix(r"\\?\")
        .map(PathBuf::from)
        .unwrap_or_else(|| path.to_path_buf());
    let Some(launch) = imp::launch(&plain, location) else {
        return Ok(false);
    };
    start(launch)?;
    Ok(true)
}

#[cfg(windows)]
mod imp {
    use super::{viewer_args, Launch, Location};
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::path::{Path, PathBuf};
    use windows_sys::Win32::UI::Shell::{AssocQueryStringW, ASSOCF_NONE, ASSOCSTR_EXECUTABLE};

    pub fn launch(path: &Path, location: Location) -> Option<Launch> {
        let extension = path.extension()?.to_str()?;
        let program = default_app(extension)?;
        let args = viewer_args(program.file_name()?.to_str()?, path, location)?;
        Some(Launch { program, args })
    }

    /// The executable Windows uses to open files with this extension (the user's choice).
    fn default_app(extension: &str) -> Option<PathBuf> {
        let wide = |text: &str| -> Vec<u16> {
            std::ffi::OsStr::new(text)
                .encode_wide()
                .chain([0])
                .collect()
        };
        let extension = wide(&format!(".{extension}"));
        let verb = wide("open");
        let mut buffer = vec![0u16; 1024];
        let mut length = buffer.len() as u32;
        // SAFETY: all strings are NUL-terminated and the buffer length is passed in/out.
        let result = unsafe {
            AssocQueryStringW(
                ASSOCF_NONE,
                ASSOCSTR_EXECUTABLE,
                extension.as_ptr(),
                verb.as_ptr(),
                buffer.as_mut_ptr(),
                &mut length,
            )
        };
        if result != 0 || length == 0 {
            return None;
        }
        let end = buffer
            .iter()
            .position(|&unit| unit == 0)
            .unwrap_or(buffer.len());
        let path = PathBuf::from(std::ffi::OsString::from_wide(&buffer[..end]));
        path.is_file().then_some(path)
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::{linux_launch, Launch, Location, XdgDirs};
    use std::path::Path;

    pub fn launch(path: &Path, location: Location) -> Option<Launch> {
        linux_launch(path, location, &XdgDirs::from_env())
    }
}

#[cfg(target_os = "macos")]
pub(crate) mod imp {
    use super::{mac_launch, Launch, Location};
    use objc2::msg_send;
    use objc2::rc::{autoreleasepool, Retained};
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2_foundation::NSString;
    use std::path::{Path, PathBuf};

    /// The default app for one file, as Launch Services reports it.
    #[derive(Debug)]
    pub struct DefaultApp {
        pub bundle_id: String,
        pub app: PathBuf,
        pub executable: PathBuf,
    }

    pub fn launch(path: &Path, location: Location) -> Option<Launch> {
        let app = default_app(path)?;
        mac_launch(&app.bundle_id, &app.app, &app.executable, path, location)
    }

    /// `NSWorkspace.urlForApplication(toOpen:)` for this file, then that app's bundle id and
    /// main executable. Read-only: nothing is opened or registered.
    pub fn default_app(path: &Path) -> Option<DefaultApp> {
        let text = path.to_str()?;
        autoreleasepool(|_| {
            let workspace_class = AnyClass::get(c"NSWorkspace")?;
            let url_class = AnyClass::get(c"NSURL")?;
            let bundle_class = AnyClass::get(c"NSBundle")?;
            let file = NSString::from_str(text);
            // SAFETY: documented selectors with object arguments and object (or nil) results;
            // every nil result is checked through Option before the next call.
            unsafe {
                let workspace: Option<Retained<AnyObject>> =
                    msg_send![workspace_class, sharedWorkspace];
                let file_url: Option<Retained<AnyObject>> =
                    msg_send![url_class, fileURLWithPath: &*file];
                let app_url: Option<Retained<AnyObject>> =
                    msg_send![&*workspace?, URLForApplicationToOpenURL: &*file_url?];
                let app_url = app_url?;
                let bundle: Option<Retained<AnyObject>> =
                    msg_send![bundle_class, bundleWithURL: &*app_url];
                let bundle = bundle?;
                let bundle_id: Option<Retained<NSString>> = msg_send![&*bundle, bundleIdentifier];
                let app_path: Option<Retained<NSString>> = msg_send![&*app_url, path];
                let executable_url: Option<Retained<AnyObject>> =
                    msg_send![&*bundle, executableURL];
                let executable: Option<Retained<NSString>> = msg_send![&*executable_url?, path];
                Some(DefaultApp {
                    bundle_id: bundle_id?.to_string(),
                    app: PathBuf::from(app_path?.to_string()),
                    executable: PathBuf::from(executable?.to_string()),
                })
            }
        })
    }
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
mod imp {
    use super::{Launch, Location};

    /// Other systems: not mapped, so results open normally.
    pub fn launch(_path: &std::path::Path, _location: Location) -> Option<Launch> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_line_of_a_trimmed_paragraph() {
        let file = "# Title\r\n\r\nFirst paragraph.\r\n\r\n  The budget grew by 18%.\r\nMore.\r\n";
        assert_eq!(line_of(file, "The budget grew by 18%.\nMore."), Some(5));
        assert_eq!(line_of(file, "\n\n# Title"), Some(1));
        assert_eq!(line_of(file, "not in the file"), None);
        assert_eq!(line_of(file, "   \n  "), None);
    }

    #[test]
    fn known_viewers_get_page_or_line_arguments() {
        let pdf = Path::new(r"C:\Docs\Q1 #2.pdf");
        assert_eq!(
            viewer_args("msedge.exe", pdf, Location::Page(3)),
            Some(vec!["file:///C:/Docs/Q1%20%232.pdf#page=3".into()])
        );
        assert_eq!(
            viewer_args("AcroRd32.exe", pdf, Location::Page(3)),
            Some(vec!["/A".into(), "page=3".into(), pdf.as_os_str().into()])
        );
        assert_eq!(
            viewer_args("SumatraPDF.exe", pdf, Location::Page(7)),
            Some(vec!["-page".into(), "7".into(), pdf.as_os_str().into()])
        );
        let text = Path::new(r"C:\Docs\notes.md");
        assert_eq!(
            viewer_args("Code.exe", text, Location::Line(12)),
            Some(vec!["--goto".into(), r"C:\Docs\notes.md:12".into()])
        );
        assert_eq!(
            viewer_args("notepad++.exe", text, Location::Line(4)),
            Some(vec!["-n4".into(), text.as_os_str().into()])
        );
    }

    #[test]
    fn linux_viewers_get_their_documented_page_or_line_options() {
        let pdf = Path::new("/home/me/보고서 #1.pdf");
        let page = |viewer| viewer_args(viewer, pdf, Location::Page(4));
        assert_eq!(
            page("evince"),
            Some(vec!["--page-index=4".into(), pdf.as_os_str().into()])
        );
        assert_eq!(
            page("okular"),
            Some(vec!["--page".into(), "4".into(), pdf.as_os_str().into()])
        );
        assert_eq!(
            page("zathura"),
            Some(vec!["--page=4".into(), pdf.as_os_str().into()])
        );
        assert_eq!(
            page("firefox"),
            Some(vec![
                "file:///home/me/%EB%B3%B4%EA%B3%A0%EC%84%9C%20%231.pdf#page=4".into()
            ])
        );
        let text = Path::new("/home/me/a b.md");
        let line = |viewer| viewer_args(viewer, text, Location::Line(9));
        assert_eq!(
            line("code"),
            Some(vec!["--goto".into(), "/home/me/a b.md:9".into()])
        );
        assert_eq!(
            line("gedit"),
            Some(vec!["+9".into(), text.as_os_str().into()])
        );
        assert_eq!(
            line("kate"),
            Some(vec!["--line".into(), "9".into(), text.as_os_str().into()])
        );
        assert_eq!(line("subl"), Some(vec!["/home/me/a b.md:9".into()]));
        // A text editor cannot take a page, a PDF viewer cannot take a line.
        assert_eq!(viewer_args("gedit", pdf, Location::Page(2)), None);
        assert_eq!(viewer_args("evince", text, Location::Line(2)), None);
        // Launchers that wrap another program are not viewers we know.
        assert_eq!(viewer_args("flatpak", pdf, Location::Page(2)), None);
        assert_eq!(viewer_args("env", text, Location::Line(2)), None);
    }

    #[test]
    fn unknown_viewers_and_mismatched_locations_fall_back() {
        let pdf = Path::new(r"C:\Docs\a.pdf");
        assert_eq!(viewer_args("SomeReader.exe", pdf, Location::Page(2)), None);
        assert_eq!(viewer_args("notepad.exe", pdf, Location::Line(2)), None);
        assert_eq!(viewer_args("Code.exe", pdf, Location::Page(2)), None);
    }

    #[test]
    fn file_urls_encode_unicode_and_reserved_characters() {
        assert_eq!(
            file_url(Path::new(r"\\?\D:\문서\a%b.pdf")),
            "file:///D:/%EB%AC%B8%EC%84%9C/a%25b.pdf"
        );
        assert_eq!(
            file_url(Path::new("/Users/me/a&b?.pdf")),
            "file:///Users/me/a%26b%3F.pdf"
        );
    }

    #[test]
    fn mac_apps_get_their_url_scheme_or_page_url() {
        let app = Path::new("/Applications/X.app");
        let exe = Path::new("/Applications/X.app/Contents/MacOS/X");
        let text = Path::new("/Users/me/메모 #2.md");
        let open = |id: &str, url: &str| {
            Some(Launch {
                program: "/usr/bin/open".into(),
                args: vec!["-b".into(), id.into(), url.into()],
            })
        };
        assert_eq!(
            mac_launch("com.microsoft.VSCode", app, exe, text, Location::Line(7)),
            open(
                "com.microsoft.VSCode",
                "vscode://file/Users/me/%EB%A9%94%EB%AA%A8%20%232.md:7"
            )
        );
        assert_eq!(
            mac_launch("com.macromates.TextMate", app, exe, text, Location::Line(7)),
            open(
                "com.macromates.TextMate",
                "txmt://open?url=file:///Users/me/%EB%A9%94%EB%AA%A8%20%232.md&line=7"
            )
        );
        let pdf = Path::new("/Users/me/a b.pdf");
        assert_eq!(
            mac_launch("com.google.Chrome", app, exe, pdf, Location::Page(3)),
            Some(Launch {
                program: exe.into(),
                args: vec!["file:///Users/me/a%20b.pdf#page=3".into()],
            })
        );
        // Preview and TextEdit have no page/line option: open normally.
        assert_eq!(
            mac_launch("com.apple.Preview", app, exe, pdf, Location::Page(3)),
            None
        );
        assert_eq!(
            mac_launch("com.apple.TextEdit", app, exe, text, Location::Line(3)),
            None
        );
        // Sublime's tool must really exist inside the app.
        assert_eq!(
            mac_launch("com.sublimetext.4", app, exe, text, Location::Line(3)),
            None
        );
        // An editor scheme is never used for a PDF page.
        assert_eq!(
            mac_launch("com.microsoft.VSCode", app, exe, pdf, Location::Page(3)),
            None
        );
    }

    #[test]
    fn mimeapps_defaults_and_desktop_exec_lines_are_read_per_spec() {
        let list = "[Added Associations]\napplication/pdf=other.desktop;\n\
                    [Default Applications]\napplication/pdf = org.gnome.Evince.desktop;okular.desktop;\n\
                    text/plain=../evil.desktop;gedit.desktop\n";
        assert_eq!(
            default_entries(list, "application/pdf"),
            ["org.gnome.Evince.desktop", "okular.desktop"]
        );
        assert_eq!(default_entries(list, "text/plain"), ["gedit.desktop"]);
        assert!(default_entries(list, "text/csv").is_empty());

        let desktop = |exec: &str| format!("[Desktop Entry]\nType=Application\nExec={exec}\n");
        assert_eq!(
            desktop_program(&desktop("evince %U")).as_deref(),
            Some("evince")
        );
        assert_eq!(
            desktop_program(&desktop(r#""/opt/My\sApps/code" --new-window %F"#)).as_deref(),
            Some("/opt/My Apps/code")
        );
        assert_eq!(
            desktop_program(&desktop(r#""/opt/a\\"b/x" %f"#)).as_deref(),
            Some(r#"/opt/a"b/x"#)
        );
        assert_eq!(desktop_program(&desktop("%U")), None);
        assert_eq!(desktop_program(&desktop("\"unterminated")), None);
        assert_eq!(
            desktop_program("[Desktop Entry]\nType=Application\nHidden=true\nExec=evince\n"),
            None
        );
        assert_eq!(
            desktop_program("[Desktop Entry]\nType=Link\nExec=evince\n"),
            None
        );
        // Exec in another group (an action) is not the app's command.
        assert_eq!(
            desktop_program("[Desktop Action new]\nExec=evil\n[Desktop Entry]\nType=Application\n"),
            None
        );
        assert_eq!(linux_mime("PDF"), Some("application/pdf"));
        assert_eq!(linux_mime("py"), None);
    }

    /// Real files on disk: the user's explicit default wins over system defaults, a missing
    /// app is skipped, and no explicit default means a normal open. Unix paths only: desktop
    /// files treat a backslash as an escape, and only Linux reads them.
    #[cfg(unix)]
    #[test]
    fn linux_default_program_follows_the_xdg_lookup_order() {
        let root = tempfile::tempdir().unwrap();
        let dir = |name: &str| {
            let path = root.path().join(name);
            std::fs::create_dir_all(&path).unwrap();
            path
        };
        let (config, system, data, bin) = (dir("config"), dir("sys"), dir("data"), dir("bin"));
        let apps = dir("sys/applications");
        let user_apps = dir("data/applications");
        for name in ["evince", "okular", "gedit"] {
            std::fs::write(bin.join(name), "").unwrap();
        }
        let entry = |exec: &Path| {
            format!(
                "[Desktop Entry]\nType=Application\nExec={} %U\n",
                exec.display()
            )
        };
        std::fs::write(
            apps.join("org.gnome.Evince.desktop"),
            entry(&bin.join("evince")),
        )
        .unwrap();
        std::fs::write(
            user_apps.join("okular.desktop"),
            "[Desktop Entry]\nType=Application\nExec=okular %U\n",
        )
        .unwrap();
        std::fs::write(
            dir("sys/applications/kde").join("gedit.desktop"),
            entry(&bin.join("gedit")),
        )
        .unwrap();
        std::fs::write(
            system.join("applications/mimeapps.list"),
            "[Default Applications]\napplication/pdf=org.gnome.Evince.desktop\ntext/plain=kde-gedit.desktop\n",
        )
        .unwrap();
        let dirs = XdgDirs {
            config_home: Some(config.clone()),
            config_dirs: vec![],
            data_home: Some(data),
            data_dirs: vec![system],
            desktops: vec!["gnome".into()],
            path: vec![bin.clone()],
        };
        // System default (data dir) when the user set nothing; "kde-" maps to kde/.
        assert_eq!(
            linux_default_program("application/pdf", &dirs),
            Some(bin.join("evince"))
        );
        assert_eq!(
            linux_default_program("text/plain", &dirs),
            Some(bin.join("gedit"))
        );
        // The user's desktop-specific choice comes first; an uninstalled app is skipped.
        std::fs::write(
            config.join("gnome-mimeapps.list"),
            "[Default Applications]\napplication/pdf=missing.desktop;okular.desktop\n",
        )
        .unwrap();
        assert_eq!(
            linux_default_program("application/pdf", &dirs),
            Some(bin.join("okular"))
        );
        // No explicit default: nothing is guessed.
        assert_eq!(linux_default_program("text/markdown", &dirs), None);

        let pdf = root.path().join("a.pdf");
        assert_eq!(
            linux_launch(&pdf, Location::Page(5), &dirs),
            Some(Launch {
                program: bin.join("okular"),
                args: vec!["--page".into(), "5".into(), pdf.clone().into()],
            })
        );
        // A known default, but a location it cannot take.
        assert_eq!(linux_launch(&pdf, Location::Line(5), &dirs), None);
    }

    /// Linux: actually starts a stand-in "evince" (a script that records its arguments),
    /// exactly as open() starts the real default app.
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_start_runs_the_default_program_with_literal_arguments() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let record = root.path().join("args.txt");
        let viewer = root.path().join("evince");
        std::fs::write(
            &viewer,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}.tmp' && mv '{0}.tmp' '{0}'\n",
                record.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&viewer, std::fs::Permissions::from_mode(0o755)).unwrap();
        let pdf = root.path().join("$(touch pwned) a;b.pdf");
        let args = viewer_args("evince", &pdf, Location::Page(2)).unwrap();
        start(Launch {
            program: viewer,
            args,
        })
        .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !record.exists() {
            assert!(std::time::Instant::now() < deadline, "viewer did not run");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let seen = std::fs::read_to_string(&record).unwrap();
        assert_eq!(seen, format!("--page-index=2\n{}\n", pdf.display()));
        assert!(!root.path().join("pwned").exists(), "no shell expansion");
    }

    /// macOS: Launch Services answers for a real file without opening anything.
    #[cfg(target_os = "macos")]
    #[test]
    fn mac_default_app_is_read_from_launch_services() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("note.txt");
        std::fs::write(&file, "hello").unwrap();
        let app = imp::default_app(&file).expect("a default app for .txt");
        assert!(!app.bundle_id.is_empty());
        assert!(
            app.app.extension().is_some_and(|ext| ext == "app"),
            "{:?}",
            app.app
        );
        assert!(app.executable.is_file(), "{:?}", app.executable);
        // TextEdit (the usual default) has no line option, so this opens normally.
        if app.bundle_id.eq_ignore_ascii_case("com.apple.TextEdit") {
            assert_eq!(imp::launch(&file, Location::Line(1)), None);
        }
    }
}
