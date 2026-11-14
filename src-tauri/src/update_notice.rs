//! "A new version is available" check for builds that cannot self-update.
//!
//! Installed builds will use the official Tauri updater (signed installers). Portable and
//! isolated builds must not replace their own files, so they only *tell* the user that a newer
//! version exists and offer to open the download page. This module does that check.
//!
//! How it works, step by step:
//! 1. Download the small `latest.json` file that every release publishes (the same file the Tauri
//!    updater reads), over HTTPS only and with a size limit.
//! 2. Read its `version` field and compare it with the running version.
//! 3. If the published version is newer, return an [`UpdateNotice`] for the UI.
//!
//! Privacy: the request carries no user data. It is a plain GET of a public file; the only
//! information sent is what any HTTP request sends (IP address and a generic User-Agent).
//! The release address is passed in by the caller, so this file contains no repository URL.

use semver::Version;
use serde::{Deserialize, Serialize};
use std::{cmp::Ordering, io::Read, time::Duration};

/// Wait this long after start-up before the first check, so start-up stays fast.
pub const FIRST_CHECK_DELAY: Duration = Duration::from_secs(30);
/// Check again at most once per day.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
/// `latest.json` is a few hundred bytes; anything larger is not a release manifest.
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
/// Release notes are shown as plain text in a small notification; keep them short.
const MAX_NOTES_CHARS: usize = 600;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// What the UI shows when a newer version exists.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateNotice {
    /// The version that is running now, e.g. "0.1.0".
    pub current: String,
    /// The newest published version, e.g. "0.2.0".
    pub latest: String,
    /// Release notes as plain text (shortened), or empty when the release has none.
    pub notes: String,
    /// Web page where the user can download the new version.
    pub download_page: String,
}

/// The part of Tauri's `latest.json` that this check needs. Other fields are ignored.
#[derive(Deserialize)]
struct Manifest {
    version: String,
    #[serde(default)]
    notes: Option<String>,
}

/// Downloads the manifest. Production uses [`HttpFetcher`]; tests use a fake so they never touch
/// the network.
pub trait ManifestFetcher {
    /// Returns the response body, or an error message that contains no response content.
    fn fetch(&self, url: &str) -> Result<Vec<u8>, String>;
}

/// Real HTTPS download with a timeout and a size limit.
pub struct HttpFetcher;

impl ManifestFetcher for HttpFetcher {
    fn fetch(&self, url: &str) -> Result<Vec<u8>, String> {
        // Only HTTPS: an attacker on the network must not be able to fake a "new version".
        let parsed = reqwest::Url::parse(url).map_err(|_| "Invalid update address.")?;
        if parsed.scheme() != "https"
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.fragment().is_some()
        {
            return Err("The update address must use HTTPS.".into());
        }
        let client = reqwest::blocking::Client::builder()
            .https_only(true)
            .timeout(REQUEST_TIMEOUT)
            .user_agent(concat!("Prism/", env!("CARGO_PKG_VERSION")))
            // GitHub's "latest release" link redirects to the actual file; a few hops are normal.
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .map_err(|_| "The update check could not start.".to_string())?;
        let response = client
            .get(url)
            .send()
            .map_err(|_| "The update server could not be reached.".to_string())?;
        if !response.status().is_success() {
            return Err(format!(
                "The update server answered with status {}.",
                response.status().as_u16()
            ));
        }
        // Read at most one byte more than the limit, so an oversized file is detected
        // without downloading all of it.
        let mut body = Vec::new();
        response
            .take(MAX_MANIFEST_BYTES + 1)
            .read_to_end(&mut body)
            .map_err(|_| "The update information could not be read.".to_string())?;
        if body.len() as u64 > MAX_MANIFEST_BYTES {
            return Err("The update information is unexpectedly large.".into());
        }
        Ok(body)
    }
}

/// Checks `manifest_url` and returns a notice when the published version is newer than
/// `current_version`. Returns `Ok(None)` when Prism is up to date (or newer, e.g. a dev build).
#[cfg(test)]
pub fn check(
    fetcher: &dyn ManifestFetcher,
    manifest_url: &str,
    download_page: &str,
    current_version: &str,
) -> Result<Option<UpdateNotice>, String> {
    let body = fetcher.fetch(manifest_url)?;
    check_bytes(&body, download_page, current_version)
}
pub fn check_bytes(
    body: &[u8],
    download_page: &str,
    current_version: &str,
) -> Result<Option<UpdateNotice>, String> {
    let current =
        Version::parse(current_version).map_err(|_| "The running version number is not valid.")?;
    if body.len() as u64 > MAX_MANIFEST_BYTES {
        return Err("The update information is unexpectedly large.".into());
    }
    let manifest: Manifest = serde_json::from_slice(body)
        .map_err(|_| "The update information is not in the expected format.".to_string())?;
    // Release tags are often written "v0.2.0"; accept both forms.
    let latest_text = manifest.version.trim();
    if latest_text.len() > 128 {
        return Err("The published version number is too long.".into());
    }
    let latest_text = latest_text.strip_prefix('v').unwrap_or(latest_text);
    let latest =
        Version::parse(latest_text).map_err(|_| "The published version number is not valid.")?;
    if latest.cmp_precedence(&current) != Ordering::Greater {
        return Ok(None);
    }
    Ok(Some(UpdateNotice {
        current: current_version.to_string(),
        latest: latest_text.to_string(),
        notes: plain_notes(manifest.notes.as_deref().unwrap_or("")),
        download_page: download_page.to_string(),
    }))
}

/// Decides whether a check is due. `last_check_ms` is when the previous check finished
/// (None = never). The caller stores it in settings so the daily limit survives restarts.
pub fn is_due(enabled: bool, last_check_ms: Option<u64>, now_ms: u64) -> bool {
    if !enabled {
        return false;
    }
    match last_check_ms {
        None => true,
        // A clock moved backwards (last > now) also counts as due, so checks never stop forever.
        Some(last) => last > now_ms || now_ms - last >= CHECK_INTERVAL.as_millis() as u64,
    }
}

/// Turns release notes into short, single-paragraph plain text: control characters are
/// removed, runs of whitespace become one space, and long notes end with "…".
/// The UI must still display it as text, never as HTML.
fn plain_notes(notes: &str) -> String {
    let mut text = String::new();
    for word in notes.split_whitespace() {
        let word: String = word.chars().filter(|ch| !ch.is_control()).collect();
        if word.is_empty() {
            continue;
        }
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(&word);
    }
    if text.chars().count() > MAX_NOTES_CHARS {
        text = text.chars().take(MAX_NOTES_CHARS - 1).collect();
        text.push('…');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// Returns a fixed body (or error) and remembers which URL was asked for.
    struct FakeFetcher {
        body: Result<Vec<u8>, String>,
        requested: RefCell<Vec<String>>,
    }
    impl FakeFetcher {
        fn json(json: &str) -> Self {
            Self {
                body: Ok(json.as_bytes().to_vec()),
                requested: RefCell::new(vec![]),
            }
        }
    }
    impl ManifestFetcher for FakeFetcher {
        fn fetch(&self, url: &str) -> Result<Vec<u8>, String> {
            self.requested.borrow_mut().push(url.to_string());
            self.body.clone()
        }
    }

    const URL: &str = "https://example.invalid/latest.json";
    const PAGE: &str = "https://example.invalid/releases";

    #[test]
    fn newer_release_produces_a_notice_with_plain_short_notes() {
        let long = "x".repeat(1000);
        let fetcher = FakeFetcher::json(&format!(
            r#"{{"version":"v0.2.0","notes":"Bug fixes\n\n- faster\u0007 search {long}","pub_date":"2026-11-04T00:00:00Z","platforms":{{}}}}"#
        ));
        let notice = check(&fetcher, URL, PAGE, "0.1.0").unwrap().unwrap();
        assert_eq!(notice.current, "0.1.0");
        assert_eq!(notice.latest, "0.2.0");
        assert_eq!(notice.download_page, PAGE);
        assert!(notice.notes.starts_with("Bug fixes - faster search x"));
        assert_eq!(notice.notes.chars().count(), MAX_NOTES_CHARS);
        assert!(notice.notes.ends_with('…'));
        assert_eq!(*fetcher.requested.borrow(), [URL]);
        let json = serde_json::to_value(&notice).unwrap();
        assert_eq!(json["downloadPage"], PAGE);
    }

    #[test]
    fn same_older_or_prerelease_versions_produce_no_notice() {
        for (published, current) in [
            ("0.1.0", "0.1.0"),
            ("0.0.9", "0.1.0"),
            ("0.2.0-beta.1", "0.2.0"),
            ("0.2.0", "0.3.0-dev"),
        ] {
            let manifest = format!(r#"{{"version":"{published}"}}"#);
            let notice = check(&FakeFetcher::json(&manifest), URL, PAGE, current).unwrap();
            assert_eq!(notice, None, "{published} vs {current}");
        }
        // A pre-release user is told about the final release.
        let manifest = r#"{"version":"0.2.0"}"#;
        assert!(check(&FakeFetcher::json(manifest), URL, PAGE, "0.2.0-rc.1")
            .unwrap()
            .is_some());
    }

    #[test]
    fn version_ordering_follows_semantic_versioning() {
        let ordered = [
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0-alpha.beta",
            "1.0.0-beta",
            "1.0.0-beta.2",
            "1.0.0-beta.11",
            "1.0.0-rc.1",
            "1.0.0",
            "1.9.0",
            "1.10.0",
            "2.0.0",
        ];
        for pair in ordered.windows(2) {
            let (a, b) = (
                Version::parse(pair[0]).unwrap(),
                Version::parse(pair[1]).unwrap(),
            );
            assert_eq!(
                a.cmp_precedence(&b),
                Ordering::Less,
                "{} < {}",
                pair[0],
                pair[1]
            );
        }
        assert_eq!(
            Version::parse("1.2.3+build.5")
                .unwrap()
                .cmp_precedence(&Version::parse("1.2.3").unwrap()),
            Ordering::Equal
        );
        for bad in [
            "",
            "1",
            "1.2",
            "1.2.3.4",
            "1.-2.3",
            "a.b.c",
            "1.2.3-",
            "1.2.3-a..b",
            "01.2.3",
            "1.2.3-01",
            "1.2.3+",
        ] {
            assert!(Version::parse(bad).is_err(), "{bad:?} must be rejected");
        }
    }

    #[test]
    fn bad_input_and_network_errors_are_reported_without_content() {
        let bad = |json: &str| check(&FakeFetcher::json(json), URL, PAGE, "0.1.0").unwrap_err();
        assert!(bad("<html>not json</html>").contains("expected format"));
        assert!(bad(r#"{"notes":"no version"}"#).contains("expected format"));
        assert!(bad(r#"{"version":"latest"}"#).contains("published version"));
        let offline = FakeFetcher {
            body: Err("The update server could not be reached.".into()),
            requested: RefCell::new(vec![]),
        };
        assert_eq!(
            check(&offline, URL, PAGE, "0.1.0").unwrap_err(),
            "The update server could not be reached."
        );
        assert!(check(&FakeFetcher::json("{}"), URL, PAGE, "dev").is_err());
        // Plain HTTP is refused before any connection is made.
        assert!(HttpFetcher
            .fetch("http://example.invalid/latest.json")
            .unwrap_err()
            .contains("HTTPS"));
    }

    #[test]
    fn bounds_and_strict_versions_apply_to_all_transports() {
        assert!(check_bytes(&vec![b'x'; 65537], PAGE, "0.1.0").is_err());
        for version in ["vv0.2.0", "01.2.3", "0.2.0-01", "0.2.0+", &"x".repeat(129)] {
            let body = serde_json::json!({"version":version}).to_string();
            assert!(check_bytes(body.as_bytes(), PAGE, "0.1.0").is_err());
        }
        assert!(
            check_bytes(br#"{"version":"0.1.0+other"}"#, PAGE, "0.1.0+build")
                .unwrap()
                .is_none()
        );
        for url in [
            "https://user:secret@example.invalid/latest.json",
            "https://example.invalid/latest.json#fragment",
        ] {
            assert!(
                HttpFetcher.fetch(url).is_err(),
                "rejected before any request"
            );
        }
    }

    #[test]
    fn checks_are_due_once_a_day_and_never_when_disabled() {
        let day = CHECK_INTERVAL.as_millis() as u64;
        assert!(is_due(true, None, 1_000));
        assert!(!is_due(true, Some(1_000), 1_000 + day - 1));
        assert!(is_due(true, Some(1_000), 1_000 + day));
        assert!(is_due(true, Some(5_000), 1_000), "clock moved backwards");
        assert!(!is_due(false, None, 1_000));
    }
}
