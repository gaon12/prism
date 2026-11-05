//! Opening a search result's original file *at the matched place* (requirement Q30).
//!
//! There is no universal "open at page N / line N" API, so this works with the app the user
//! already chose as the default for that file type:
//! 1. Work out where the match is: a PDF page, or a line in a plain-text file.
//! 2. Find the default app for the extension (Windows file associations).
//! 3. If that app is one we know accepts a page/line argument, start it with that argument.
//!    Otherwise the caller opens the file normally, exactly as before.
//!
//! Nothing here invents a viewer the user did not choose, and nothing is run through a shell.

use std::ffi::OsString;
use std::path::Path;

/// Where in the file the match is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Location {
    /// 1-based PDF page.
    Page(u32),
    /// 1-based line in a text file.
    Line(usize),
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

/// Arguments that make `viewer` (the default app's file name, e.g. "Code.exe") open `path`
/// at `location`. None when that viewer is unknown or cannot jump to this kind of location.
pub fn viewer_args(viewer: &str, path: &Path, location: Location) -> Option<Vec<OsString>> {
    let name = viewer.to_ascii_lowercase();
    let file = path.as_os_str().to_os_string();
    match location {
        Location::Page(page) => match name.as_str() {
            // Browsers' built-in PDF viewers read the page from the URL fragment.
            "msedge.exe" | "chrome.exe" | "firefox.exe" | "brave.exe" => {
                Some(vec![format!("{}#page={page}", file_url(path)).into()])
            }
            // Adobe Acrobat / Reader: /A "page=N" <file>.
            "acrobat.exe" | "acrord32.exe" => {
                Some(vec!["/A".into(), format!("page={page}").into(), file])
            }
            "sumatrapdf.exe" => Some(vec!["-page".into(), page.to_string().into(), file]),
            _ => None,
        },
        Location::Line(line) => match name.as_str() {
            "code.exe" | "code - insiders.exe" | "cursor.exe" => {
                let mut target = file;
                target.push(format!(":{line}"));
                Some(vec!["--goto".into(), target])
            }
            "notepad++.exe" => Some(vec![format!("-n{line}").into(), file]),
            "sublime_text.exe" => {
                let mut target = file;
                target.push(format!(":{line}"));
                Some(vec![target])
            }
            _ => None,
        },
    }
}

/// `file:///C:/My%20Docs/a.pdf` for a local path, percent-encoding everything that is not a
/// plain URL character (so `#`, `%`, `?`, spaces and non-ASCII names survive).
fn file_url(path: &Path) -> String {
    let text = path.to_string_lossy();
    let text = text
        .strip_prefix(r"\\?\")
        .unwrap_or(&text)
        .replace('\\', "/");
    let mut url = String::from("file:///");
    for byte in text.trim_start_matches('/').bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/:".contains(&byte) {
            url.push(byte as char);
        } else {
            url.push_str(&format!("%{byte:02X}"));
        }
    }
    url
}

/// Starts the default app for `path` at `location` when it supports that. Returns Ok(true)
/// when started, Ok(false) when the caller should fall back to a normal open.
pub fn open(path: &Path, location: Location) -> std::io::Result<bool> {
    let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
        return Ok(false);
    };
    let Some(viewer) = imp::default_app(extension) else {
        return Ok(false);
    };
    let Some(name) = viewer.file_name().and_then(|name| name.to_str()) else {
        return Ok(false);
    };
    let plain = path
        .to_string_lossy()
        .strip_prefix(r"\\?\")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| path.to_path_buf());
    let Some(args) = viewer_args(name, &plain, location) else {
        return Ok(false);
    };
    std::process::Command::new(&viewer).args(args).spawn()?;
    Ok(true)
}

#[cfg(windows)]
mod imp {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::path::PathBuf;
    use windows_sys::Win32::UI::Shell::{AssocQueryStringW, ASSOCF_NONE, ASSOCSTR_EXECUTABLE};

    /// The executable Windows uses to open files with this extension (the user's choice).
    pub fn default_app(extension: &str) -> Option<PathBuf> {
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

#[cfg(not(windows))]
mod imp {
    /// macOS/Linux: not mapped yet, so results open normally.
    pub fn default_app(_extension: &str) -> Option<std::path::PathBuf> {
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
    }
}
