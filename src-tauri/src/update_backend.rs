//! Production update transport. No document or user identifiers enter requests.
use crate::{
    app_paths::DataLocation,
    update_notice::{self, ManifestFetcher},
    update_service::{AvailableUpdate, UpdateBackend},
};
use std::{io::Read, sync::Mutex, time::Duration};
use tauri::AppHandle;
use tauri_plugin_updater::{Update, UpdaterExt};

/// Address of the update server (server/update/index.php), fixed when Prism is built:
/// `PRISM_UPDATE_URL=https://example.com/prism/update/index.php`. It answers with the latest
/// version, each package's download address, SHA-256 hash and signature. A build without it
/// has no update server and says so when checking. Not a user setting, so nothing on the PC can
/// redirect updates to another server.
pub const MANIFEST_URL: Option<&str> = option_env!("PRISM_UPDATE_URL");
/// Page opened for manual downloads. `PRISM_DOWNLOAD_PAGE` at build time, else GitHub releases.
pub const DOWNLOAD_PAGE: &str = match option_env!("PRISM_DOWNLOAD_PAGE") {
    Some(page) => page,
    None => "https://github.com/gaon12/prism/releases/latest",
};
pub const PUBLIC_KEY: &str = match option_env!("PRISM_UPDATER_PUBLIC_KEY") {
    Some(key) => key,
    None => "",
};
const MAX_PACKAGE_BYTES: u64 = 512 * 1024 * 1024;

pub struct NativeBackend {
    app: AppHandle,
    eligible: bool,
    cached: Mutex<Option<Update>>,
}
impl NativeBackend {
    pub fn new(app: AppHandle, location: &DataLocation) -> Self {
        Self {
            app,
            eligible: location.allows_installer_update()
                && !cfg!(debug_assertions)
                && !PUBLIC_KEY.trim().is_empty(),
            cached: Mutex::new(None),
        }
    }
}
pub fn https_url(url: &reqwest::Url) -> bool {
    url.scheme() == "https"
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
}
impl UpdateBackend for NativeBackend {
    fn check(&self) -> Result<Option<AvailableUpdate>, String> {
        let url = MANIFEST_URL
            .map(str::trim)
            .filter(|url| !url.is_empty())
            .ok_or("This build has no update server configured.")?;
        // The fetcher itself refuses anything but plain HTTPS addresses.
        let bytes = update_notice::HttpFetcher.fetch(url)?;
        let Some(notice) =
            update_notice::check_bytes(&bytes, DOWNLOAD_PAGE, env!("CARGO_PKG_VERSION"))?
        else {
            *self
                .cached
                .lock()
                .map_err(|_| "Update state unavailable.")? = None;
            return Ok(None);
        };
        if self.eligible {
            let raw = serde_json::from_slice(&bytes).map_err(|_| "Invalid update information.")?;
            let updater = self
                .app
                .updater_builder()
                .timeout(Duration::from_secs(10))
                .build()
                .map_err(|_| "Updater configuration is unavailable.")?;
            let update = updater
                .restore_update(raw)
                .map_err(|_| "No compatible signed update package is available.")?;
            if let Some(update) = &update {
                if !https_url(&update.download_url) {
                    return Err(
                        "Update package must use an HTTPS address without credentials.".into(),
                    );
                }
            }
            *self
                .cached
                .lock()
                .map_err(|_| "Update state unavailable.")? = update;
        }
        Ok(Some(AvailableUpdate {
            version: notice.latest,
            notes: notice.notes,
        }))
    }
    fn can_install(&self) -> bool {
        self.eligible && self.cached.lock().is_ok_and(|cached| cached.is_some())
    }
    fn install(
        &self,
        version: &str,
        prepare: &dyn Fn() -> Result<(), String>,
    ) -> Result<(), String> {
        if !self.eligible {
            return Err("This build uses manual downloads for updates.".into());
        }
        let update = self
            .cached
            .lock()
            .map_err(|_| "Update state unavailable.")?
            .clone()
            .filter(|update| update.version == version)
            .ok_or("The selected update changed. Check again.")?;
        if !https_url(&update.download_url) {
            return Err("Invalid update package address.".into());
        }
        let client = reqwest::blocking::Client::builder()
            .https_only(true)
            .timeout(Duration::from_secs(300))
            .redirect(reqwest::redirect::Policy::limited(5))
            .user_agent(concat!("Prism/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| "Could not start update download.")?;
        let response = client
            .get(update.download_url.as_str())
            .send()
            .map_err(|_| "Could not download the update.")?;
        if !response.status().is_success() {
            return Err("The update download failed.".into());
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_PACKAGE_BYTES)
        {
            return Err("The update package is too large.".into());
        }
        let mut bytes = vec![];
        response
            .take(MAX_PACKAGE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "Could not read the update package.")?;
        if bytes.len() as u64 > MAX_PACKAGE_BYTES {
            return Err("The update package is too large.".into());
        }
        // The server's file hash: catches a damaged or swapped download early. The signature
        // below still decides authenticity, since the hash comes from the same server.
        check_package_hash(&update.raw_json, &update.download_url, &bytes)?;
        // Signature AND signed-version validation precede all application preparation.
        update
            .verify(&bytes)
            .map_err(|_| "The update signature or signed version could not be verified.")?;
        prepare()?;
        update
            .install(bytes)
            .map_err(|_| "The verified update could not be installed.")?;
        #[cfg(not(windows))]
        self.app.restart();
        #[cfg(windows)]
        Ok(())
    }
}

/// Compares the downloaded package with the `sha256` the update server lists for it.
///
/// The entry is found by its download address (the one the updater picked for this PC), so the
/// hash always belongs to exactly this file. A missing or malformed hash refuses the update.
pub fn check_package_hash(
    manifest: &serde_json::Value,
    download_url: &reqwest::Url,
    bytes: &[u8],
) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    let expected = manifest["platforms"]
        .as_object()
        .and_then(|platforms| {
            platforms.values().find(|entry| {
                // Compare parsed addresses, so spelling differences such as an upper-case
                // host name do not matter.
                entry["url"]
                    .as_str()
                    .and_then(|url| reqwest::Url::parse(url).ok())
                    .as_ref()
                    == Some(download_url)
            })
        })
        .and_then(|entry| entry["sha256"].as_str())
        .filter(|hash| {
            hash.len() == 64 && hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        })
        .ok_or("The update information has no valid file hash for this package.")?;
    let actual: String = Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if actual != expected {
        return Err("The downloaded update does not match its published hash.".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const URL: &str = "https://updates.example.com/prism/Prism_0.2.0_x64-setup.exe";
    /// SHA-256 of the bytes `abc`.
    const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    fn manifest(url: &str, sha256: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "version": "0.2.0",
            "platforms": {
                "linux-x86_64": { "url": "https://other.example.com/a", "sha256": "0".repeat(64) },
                "windows-x86_64": { "url": url, "signature": "s", "sha256": sha256 },
            }
        })
    }

    /// Checks `bytes` as if downloaded from `download`.
    fn check(manifest: &serde_json::Value, download: &str, bytes: &[u8]) -> Result<(), String> {
        check_package_hash(manifest, &reqwest::Url::parse(download).unwrap(), bytes)
    }

    #[test]
    fn the_package_must_match_the_hash_listed_for_its_own_address() {
        assert_eq!(check(&manifest(URL, ABC.into()), URL, b"abc"), Ok(()));
        // The same address spelled with an upper-case host still finds its entry.
        let upper = URL.replace("updates.example.com", "UPDATES.example.com");
        assert_eq!(check(&manifest(&upper, ABC.into()), URL, b"abc"), Ok(()));
        // Changed bytes.
        assert!(check(&manifest(URL, ABC.into()), URL, b"abd")
            .unwrap_err()
            .contains("does not match"));
        // Another platform's hash is never used for this download.
        assert!(check(
            &manifest(URL, ABC.into()),
            "https://x.example.com/b",
            b"abc"
        )
        .is_err());
    }

    #[test]
    fn missing_or_malformed_hashes_refuse_the_update() {
        for hash in [
            serde_json::Value::Null,
            ABC.to_uppercase().into(),
            ABC[..63].into(),
            serde_json::json!(5),
        ] {
            assert!(check(&manifest(URL, hash), URL, b"abc")
                .unwrap_err()
                .contains("no valid file hash"));
        }
        assert!(check(&serde_json::json!({}), URL, b"abc").is_err());
    }
}
