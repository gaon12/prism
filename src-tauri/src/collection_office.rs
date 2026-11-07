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
    installation: Mutex<Option<Arc<AtomicBool>>>,
    stopping: AtomicBool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub supported: bool,
    /// Presence, not a successful integrity or executable-startup check.
    pub installed: bool,
    pub installing: bool,
    pub archive_bytes: u64,
    pub expanded_bytes: u64,
    pub download_available: bool,
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
            installation: Mutex::new(None),
            stopping: AtomicBool::new(false),
        })
    }
    pub fn status(&self) -> Result<Status> {
        let catalog = office_assets::catalog().map_err(map)?;
        Ok(Status {
            supported: cfg!(all(windows, target_arch = "x86_64")),
            installed: self.runtime.installed().map_err(map)?,
            installing: self
                .installation
                .lock()
                .map_err(|_| error(ErrorCode::Storage))?
                .is_some(),
            archive_bytes: catalog.archive.bytes,
            expanded_bytes: catalog.expanded_bytes,
            download_available: catalog.archive.url.is_some(),
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
        if !cfg!(all(windows, target_arch = "x86_64")) {
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
            if active.is_some() {
                return Err(error(ErrorCode::Busy));
            }
            *active = Some(cancel.clone());
        }
        // Clear the active token on success, errors and unwinding.
        struct Install<'a>(&'a Mutex<Option<Arc<AtomicBool>>>);
        impl Drop for Install<'_> {
            fn drop(&mut self) {
                *self.0.lock().unwrap_or_else(|e| e.into_inner()) = None;
            }
        }
        let _active = Install(&self.installation);
        let deadline = Instant::now() + Duration::from_secs(600);
        let mut cache = self.lock(&cancel, deadline)?;
        self.runtime
            .install_archive(path, &cancel, deadline, progress)
            .map_err(map)?;
        cache.entries.clear();
        cache.bytes = 0;
        Ok(())
    }
    pub fn cancel_install(&self) {
        if let Ok(active) = self.installation.lock() {
            if let Some(cancel) = active.as_ref() {
                cancel.store(true, Ordering::Release);
            }
        }
    }
    pub fn shutdown(&self) {
        self.stopping.store(true, Ordering::Release);
        self.cancel_install();
    }
    pub fn check_page(&self, page: &Page) -> Result<()> {
        page.location.validate()?;
        if page.location.kind != PageKind::Office
            || page.location.renderer.as_deref() != Some(self.runtime.fingerprint().as_str())
        {
            return Err(error(ErrorCode::SourceChanged));
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
