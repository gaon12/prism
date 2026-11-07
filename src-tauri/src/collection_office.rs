//! Original Office pages use the verified local engine, never extracted text
//! disguised as document layout. Only generated PDFs enter this bounded RAM cache.
use crate::{
    collection_import as import,
    collection_store::{
        error, ErrorCode, Page, PageKind, PageLocation, Result, Source, SourceKind,
    },
    office_assets, office_preview,
};
use serde::Serialize;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, MutexGuard, TryLockError,
    },
    time::{Duration, Instant},
};

const BUDGET: usize = 128 << 20;
struct Cached {
    pdf: Arc<Vec<u8>>,
    used: u64,
}
#[derive(Default)]
struct Cache {
    entries: HashMap<String, Cached>,
    bytes: usize,
    clock: u64,
}
pub struct Office {
    runtime: office_preview::Runtime,
    cache: Mutex<Cache>,
    installation: Mutex<Installation>,
    stopping: AtomicBool,
}
#[derive(Default)]
struct Installation {
    cancel: Option<Arc<AtomicBool>>,
    progress: Option<office_preview::Progress>,
    last_error: Option<ErrorCode>,
    failure_details: Option<FailureDetails>,
    revision: u64,
}
/// Fixed operation tokens and native numbers only, never paths or document data.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FailureDetails {
    pub stage: Option<&'static str>,
    pub native_code: Option<i32>,
}
impl FailureDetails {
    fn from_issue(issue: &office_assets::Error) -> Option<Self> {
        (issue.stage.is_some() || issue.native_code.is_some()).then_some(Self {
            stage: issue.stage,
            native_code: issue.native_code,
        })
    }
}
// The snapshot survives a settings-window remount. Revision prevents an older
// status response from overwriting a newer notification in either app window.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub supported: bool,
    /// Presence, not a successful integrity or executable-startup check.
    pub installed: bool,
    pub installing: bool,
    pub archive_bytes: u64,
    pub expanded_bytes: u64,
    pub download_available: bool,
    pub progress: Option<office_preview::Progress>,
    pub last_error: Option<ErrorCode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_details: Option<FailureDetails>,
    pub revision: u64,
}
pub fn map(issue: office_assets::Error) -> crate::collection_store::LibraryError {
    use office_assets::Code;
    error(match issue.code {
        Code::Unavailable => ErrorCode::RendererUnavailable,
        Code::Integrity => ErrorCode::RendererIntegrity,
        Code::Cancelled => ErrorCode::Cancelled,
        Code::TimedOut => ErrorCode::TimedOut,
        Code::Busy => ErrorCode::Busy,
        Code::Encrypted => ErrorCode::Encrypted,
        Code::Unsupported => ErrorCode::Unsupported,
        Code::Limit => ErrorCode::Limit,
        Code::Input => ErrorCode::Input,
        Code::Protocol | Code::Failed => ErrorCode::RendererFailed,
    })
}
impl Office {
    pub fn open(directory: PathBuf) -> Result<Self> {
        Ok(Self {
            runtime: office_preview::Runtime::open(directory).map_err(map)?,
            cache: Mutex::new(Cache::default()),
            installation: Mutex::new(Installation::default()),
            stopping: AtomicBool::new(false),
        })
    }
    pub fn status(&self) -> Result<Status> {
        let catalog = office_assets::catalog().map_err(map)?;
        let installation = self
            .installation
            .lock()
            .map_err(|_| error(ErrorCode::Storage))?;
        Ok(Status {
            supported: office_preview::supported(),
            installed: self.runtime.installed().map_err(map)?,
            installing: installation.cancel.is_some(),
            archive_bytes: catalog.archive.bytes,
            expanded_bytes: catalog.expanded_bytes,
            download_available: catalog.archive.url.is_some(),
            progress: installation.progress.clone(),
            last_error: installation.last_error.clone(),
            failure_details: installation.failure_details.clone(),
            revision: installation.revision,
        })
    }
    fn lock(&self, cancel: &AtomicBool, deadline: Instant) -> Result<MutexGuard<'_, Cache>> {
        loop {
            import::check(cancel)?;
            if Instant::now() >= deadline {
                return Err(error(ErrorCode::TimedOut));
            }
            match self.cache.try_lock() {
                Ok(guard) => return Ok(guard),
                Err(TryLockError::Poisoned(_)) => return Err(error(ErrorCode::Storage)),
                Err(TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(20)),
            }
        }
    }
    pub fn install(&self, path: &Path, progress: &dyn Fn(office_preview::Progress)) -> Result<()> {
        if !office_preview::supported() {
            return Err(error(ErrorCode::Unsupported));
        }
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let mut active = self
                .installation
                .lock()
                .map_err(|_| error(ErrorCode::Storage))?;
            if self.stopping.load(Ordering::Acquire) {
                return Err(error(ErrorCode::Cancelled));
            }
            if active.cancel.is_some() {
                return Err(error(ErrorCode::Busy));
            }
            active.cancel = Some(cancel.clone());
            active.progress = None;
            active.last_error = None;
            active.failure_details = None;
            active.revision += 1;
        }
        // Clear the active token on success, errors and unwinding.
        struct Install<'a>(
            &'a Mutex<Installation>,
            Option<ErrorCode>,
            Option<FailureDetails>,
        );
        impl Drop for Install<'_> {
            fn drop(&mut self) {
                let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
                state.cancel = None;
                state.last_error = self.1.clone();
                state.failure_details = self.2.clone();
                state.revision += 1;
            }
        }
        let mut active = Install(&self.installation, Some(ErrorCode::Storage), None);
        let deadline = Instant::now() + Duration::from_secs(600);
        let report = |value: office_preview::Progress| {
            {
                let mut state = self.installation.lock().unwrap_or_else(|e| e.into_inner());
                state.progress = Some(value.clone());
                state.revision += 1;
            }
            progress(value);
        };
        report(office_preview::Progress {
            phase: "waiting",
            files: 0,
            total_files: 0,
        });
        let mut failure_details = None;
        let result = (|| {
            let mut cache = self.lock(&cancel, deadline)?;
            self.runtime
                .install_archive(path, &cancel, deadline, &report)
                .map_err(|issue| {
                    failure_details = FailureDetails::from_issue(&issue);
                    map(issue)
                })?;
            cache.entries.clear();
            cache.bytes = 0;
            Ok(())
        })();
        active.1 = result
            .as_ref()
            .err()
            .map(|issue: &crate::collection_store::LibraryError| issue.code.clone());
        active.2 = failure_details;
        result
    }
    pub fn cancel_install(&self) {
        if let Ok(active) = self.installation.lock() {
            if let Some(cancel) = active.cancel.as_ref() {
                cancel.store(true, Ordering::Release);
            }
        }
    }
    pub fn shutdown(&self) {
        self.stopping.store(true, Ordering::Release);
        self.cancel_install();
    }
    pub fn fingerprint(&self) -> String {
        self.runtime.fingerprint()
    }
    pub fn check_page(&self, page: &Page) -> Result<()> {
        page.location.validate()?;
        if page.location.kind != PageKind::Office
            || page.location.renderer.as_deref() != Some(self.runtime.fingerprint().as_str())
        {
            return Err(error(ErrorCode::RendererChanged));
        }
        Ok(())
    }
    pub fn pdf(
        &self,
        bytes: &[u8],
        extension: &str,
        cancel: &AtomicBool,
        progress: &dyn Fn(office_preview::Progress),
    ) -> Result<Arc<Vec<u8>>> {
        if bytes.is_empty() || bytes.len() > office_assets::MAX_DOCUMENT {
            return Err(error(ErrorCode::Limit));
        }
        let deadline = Instant::now() + Duration::from_secs(120);
        let mut cache = self.lock(cancel, deadline)?;
        let key = format!(
            "{extension}:{}:{}",
            import::hash(bytes),
            self.runtime.fingerprint()
        );
        cache.clock = cache.clock.wrapping_add(1);
        let clock = cache.clock;
        if let Some(entry) = cache.entries.get_mut(&key) {
            entry.used = clock;
            return Ok(entry.pdf.clone());
        }
        let pdf = Arc::new(
            self.runtime
                .render_pdf(extension, bytes, cancel, deadline, progress)
                .map_err(map)?,
        );
        // Remove least-recently used entries before adding a new document. The
        // importer retains only page metadata; media callers are serialized.
        while cache.bytes.saturating_add(pdf.len()) > BUDGET || cache.entries.len() >= 8 {
            let Some(oldest) = cache
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.used)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            if let Some(entry) = cache.entries.remove(&oldest) {
                cache.bytes -= entry.pdf.len();
            }
        }
        cache.bytes += pdf.len();
        cache.entries.insert(
            key,
            Cached {
                pdf: pdf.clone(),
                used: clock,
            },
        );
        Ok(pdf)
    }
    pub fn manifest(
        &self,
        source: &Source,
        excluded: &Path,
        cancel: &AtomicBool,
        progress: &dyn Fn(office_preview::Progress),
    ) -> Result<Option<Vec<Page>>> {
        let extension = extension(source);
        if source.kind != SourceKind::Document
            || !matches!(extension.as_str(), "docx" | "pptx" | "xlsx")
            || !self.runtime.installed().map_err(map)?
        {
            return Ok(None);
        }
        let bytes = import::read(Path::new(&source.path), excluded, cancel)?;
        let source_hash = import::hash(&bytes);
        let pdf = self.pdf(&bytes, &extension, cancel, progress)?;
        let document =
            pdf_extract::Document::load_mem(&pdf).map_err(|_| error(ErrorCode::Unsupported))?;
        let count = document.get_pages().len();
        if count == 0 || count > 250_000 {
            return Err(error(ErrorCode::Limit));
        }
        // Rendering can be slow. Authenticate the current original again before
        // committing any page count or identities derived from the old bytes.
        if import::hash(&import::read(Path::new(&source.path), excluded, cancel)?) != source_hash {
            return Err(error(ErrorCode::SourceChanged));
        }
        let renderer = self.runtime.fingerprint();
        let pages = (1..=count as u32)
            .map(|number| {
                import::page_location(
                    source,
                    PageLocation {
                        kind: PageKind::Office,
                        entry: None,
                        number,
                        renderer: Some(renderer.clone()),
                    },
                    number.to_string(),
                    &source_hash,
                )
            })
            .collect();
        Ok(Some(pages))
    }
}
pub fn extension(source: &Source) -> String {
    Path::new(&source.path)
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase()
}

#[cfg(test)]
mod failure_details_tests {
    use super::*;

    #[test]
    fn installation_diagnostics_serialize_fixed_tokens_and_numbers_only() {
        assert!(
            FailureDetails::from_issue(&office_assets::error(office_assets::Code::Integrity))
                .is_none()
        );
        let details = FailureDetails::from_issue(&office_assets::Error {
            code: office_assets::Code::Failed,
            stage: Some("promote"),
            native_code: Some(5),
        })
        .unwrap();
        assert_eq!(
            serde_json::to_value(details).unwrap(),
            serde_json::json!({"stage":"promote", "nativeCode":5})
        );
    }
}

#[cfg(all(test, windows, target_arch = "x86_64"))]
mod tests {
    use super::*;

    #[test]
    fn failed_installation_retains_a_terminal_snapshot_without_claiming_presence() {
        let directory = tempfile::tempdir().unwrap();
        let office = Office::open(directory.path().join("engine")).unwrap();
        let before = office.status().unwrap();
        assert!(!before.installed && !before.installing);
        let failure = office
            .install(&directory.path().join("missing.zip"), &|_| {})
            .unwrap_err();
        let after = office.status().unwrap();
        assert!(!after.installed && !after.installing);
        assert_eq!(after.last_error, Some(failure.code));
        assert!(after.revision > before.revision);
        let json = serde_json::to_value(&after).unwrap();
        assert!(json.get("lastError").is_some());
        assert!(json.get("archiveBytes").is_some());
        assert_eq!(office.status().unwrap().revision, after.revision);
    }

    #[test]
    fn waiting_installation_can_cancel_and_busy_requests_do_not_replace_its_state() {
        let directory = tempfile::tempdir().unwrap();
        let office = Office::open(directory.path().join("engine")).unwrap();
        // A real reader holds the shared cache while an install waits. No OS
        // process, user profile or archive is involved in this admission test.
        let reader = office.cache.lock().unwrap();
        std::thread::scope(|scope| {
            let (sender, receiver) = std::sync::mpsc::sync_channel(1);
            let installer = &office;
            let archive = directory.path().join("missing.zip");
            let worker = scope.spawn(move || {
                installer.install(&archive, &|p| {
                    assert_eq!(p.phase, "waiting");
                    sender.send(()).unwrap();
                })
            });
            receiver.recv_timeout(Duration::from_secs(5)).unwrap();
            let waiting = office.status().unwrap();
            assert!(waiting.installing);
            assert_eq!(waiting.progress.as_ref().unwrap().phase, "waiting");
            assert_eq!(
                office
                    .install(&directory.path().join("another.zip"), &|_| {})
                    .unwrap_err()
                    .code,
                ErrorCode::Busy
            );
            assert_eq!(office.status().unwrap().revision, waiting.revision);
            office.cancel_install();
            assert_eq!(
                worker.join().unwrap().unwrap_err().code,
                ErrorCode::Cancelled
            );
            let terminal = office.status().unwrap();
            assert!(!terminal.installing);
            assert_eq!(terminal.last_error, Some(ErrorCode::Cancelled));
            assert!(terminal.revision > waiting.revision);
        });
        drop(reader);
    }
}
