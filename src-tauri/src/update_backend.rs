//! Production update transport. No document or user identifiers enter requests.
use crate::{
    app_paths::DataLocation,
    update_notice::{self, ManifestFetcher},
    update_service::{AvailableUpdate, UpdateBackend},
};
use std::{io::Read, sync::Mutex, time::Duration};
use tauri::AppHandle;
use tauri_plugin_updater::{Update, UpdaterExt};

pub const MANIFEST_URL: &str =
    "https://github.com/gaon12/prism/releases/latest/download/latest.json";
pub const DOWNLOAD_PAGE: &str = "https://github.com/gaon12/prism/releases/latest";
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
        let bytes = update_notice::HttpFetcher.fetch(MANIFEST_URL)?;
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
