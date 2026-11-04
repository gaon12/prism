//! Online-first lookup and explicit offline-pack installation. All methods that
//! perform I/O must run on a blocking worker; construction never downloads data.
use crate::dictionary_provider::{ProviderError, WiktionaryProvider};
use crate::dictionary_store::{
    Definition, DictionaryStore, InstalledPack, PackManifest, LANGUAGES,
};
use serde::Serialize;
use std::{
    collections::HashMap,
    fs::{self, File},
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use unicode_normalization::UnicodeNormalization;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DictionaryError {
    pub code: String,
    pub message: String,
}
impl From<ProviderError> for DictionaryError {
    fn from(error: ProviderError) -> Self {
        Self {
            code: serde_json::to_value(error.code)
                .ok()
                .and_then(|v| v.as_str().map(String::from))
                .unwrap_or_else(|| "network".into()),
            message: error.message,
        }
    }
}
fn error(code: &str, message: &str) -> DictionaryError {
    DictionaryError {
        code: code.into(),
        message: message.into(),
    }
}
type Result<T> = std::result::Result<T, DictionaryError>;
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LookupResponse {
    pub word: String,
    pub language: String,
    pub definitions: Vec<Definition>,
    pub source: String,
    pub offline_reason: Option<String>,
    pub offline_pack_installed: bool,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackJob {
    pub phase: String,
    pub language: Option<String>,
    pub bytes: u64,
    pub total_bytes: Option<u64>,
    pub entries: u64,
    pub error: Option<DictionaryError>,
}
impl Default for PackJob {
    fn default() -> Self {
        Self {
            phase: "idle".into(),
            language: None,
            bytes: 0,
            total_bytes: None,
            entries: 0,
            error: None,
        }
    }
}
type Cache = HashMap<(String, String), (Instant, Vec<Definition>)>;
#[derive(Clone)]
pub struct DictionaryService {
    store: DictionaryStore,
    provider: WiktionaryProvider,
    directory: PathBuf,
    cache: Arc<Mutex<Cache>>,
    job: Arc<Mutex<PackJob>>,
    operation: Arc<Mutex<()>>,
    cancel: Arc<AtomicBool>,
}
impl DictionaryService {
    pub fn open(directory: &Path) -> Result<Self> {
        Ok(Self {
            store: DictionaryStore::open(directory)
                .map_err(|message| error("storage", &message))?,
            provider: WiktionaryProvider::new()?,
            directory: directory.into(),
            cache: Arc::new(Mutex::new(HashMap::new())),
            job: Arc::new(Mutex::new(PackJob::default())),
            operation: Arc::new(Mutex::new(())),
            cancel: Arc::new(AtomicBool::new(false)),
        })
    }
    pub fn lookup(&self, language: &str, word: &str, offline: bool) -> Result<LookupResponse> {
        self.lookup_with(language, word, offline, |language, word| {
            self.provider.lookup(language, word)
        })
    }
    fn lookup_with(
        &self,
        language: &str,
        word: &str,
        offline: bool,
        provider: impl FnOnce(&str, &str) -> std::result::Result<Vec<Definition>, ProviderError>,
    ) -> Result<LookupResponse> {
        let word: String = word.trim().nfc().collect();
        if !LANGUAGES.contains(&language)
            || word.is_empty()
            || word.chars().count() > 256
            || word.chars().any(char::is_control)
        {
            return Err(error(
                "input",
                "Enter a word up to 256 characters and choose a supported language.",
            ));
        }
        let installed = self
            .packs()?
            .iter()
            .any(|pack| pack.manifest.language == language);
        let response = |definitions, source: &str, offline_reason| LookupResponse {
            word: word.clone(),
            language: language.into(),
            definitions,
            source: source.into(),
            offline_reason,
            offline_pack_installed: installed,
        };
        let key = (language.into(), word.clone());
        if !offline {
            if let Some((_, definitions)) = self
                .cache
                .lock()
                .map_err(|_| error("storage", "Dictionary cache unavailable."))?
                .get(&key)
                .filter(|(at, _)| at.elapsed() < Duration::from_secs(3600))
            {
                return Ok(response(definitions.clone(), "onlineCache", None));
            }
            match provider(language, &word) {
                Ok(definitions) => {
                    let mut cache = self
                        .cache
                        .lock()
                        .map_err(|_| error("storage", "Dictionary cache unavailable."))?;
                    cache.retain(|_, (at, _)| at.elapsed() < Duration::from_secs(3600));
                    if cache.len() >= 256 {
                        if let Some(oldest) = cache
                            .iter()
                            .min_by_key(|(_, (at, _))| *at)
                            .map(|(key, _)| key.clone())
                        {
                            cache.remove(&oldest);
                        }
                    }
                    cache.insert(key, (Instant::now(), definitions.clone()));
                    return Ok(response(definitions, "online", None));
                }
                Err(error) if error.fallback_allowed() => {}
                Err(error) => return Err(error.into()),
            }
        }
        let definitions = self
            .store
            .lookup(language, &word)
            .map_err(|message| error("storage", &message))?;
        Ok(response(
            definitions,
            "offline",
            Some(if offline { "requested" } else { "network" }.into()),
        ))
    }
    pub fn packs(&self) -> Result<Vec<InstalledPack>> {
        self.store
            .packs()
            .map_err(|message| error("storage", &message))
    }
    pub fn job(&self) -> Result<PackJob> {
        self.job
            .lock()
            .map(|job| job.clone())
            .map_err(|_| error("storage", "Dictionary installation state unavailable."))
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }
    pub fn remove(&self, id: &str) -> Result<()> {
        let _operation = self
            .operation
            .try_lock()
            .map_err(|_| error("busy", "Wait for the current pack installation to finish."))?;
        self.store
            .remove(id)
            .map_err(|message| error("storage", &message))
    }
    pub fn import_pack(
        &self,
        path: &Path,
        manifest: Option<PackManifest>,
    ) -> Result<InstalledPack> {
        let _operation = self
            .operation
            .try_lock()
            .map_err(|_| error("busy", "Another dictionary installation is running."))?;
        self.cancel.store(false, Ordering::Release);
        self.set_job(PackJob {
            phase: "importing".into(),
            ..Default::default()
        });
        let result = (|| {
            if !path.is_absolute() || !fs::metadata(path).is_ok_and(|metadata| metadata.is_file()) {
                return Err(error(
                    "input",
                    "Choose a local dictionary JSONL or gzip file.",
                ));
            }
            let file = File::open(path)
                .map_err(|_| error("input", "The selected dictionary file is unavailable."))?;
            let mut buffered = BufReader::new(file);
            let gzip = buffered
                .fill_buf()
                .map_err(|_| error("input", "Could not read the selected dictionary file."))?
                .starts_with(&[0x1f, 0x8b]);
            let mut reader: Box<dyn BufRead> = if gzip {
                Box::new(BufReader::new(flate2::read::GzDecoder::new(buffered)))
            } else {
                Box::new(buffered)
            };
            let manifest = match manifest {
                Some(manifest) => manifest,
                None => {
                    let mut first = String::new();
                    (&mut reader)
                        .take(65537)
                        .read_line(&mut first)
                        .map_err(|_| error("input", "Could not read dictionary metadata."))?;
                    if first.len() > 65536 {
                        return Err(error("input", "Dictionary metadata exceeds 64 KiB."));
                    }
                    serde_json::from_str::<serde_json::Value>(first.trim_start_matches('\u{feff}')).ok().and_then(|value| value.get("prismDictionary").cloned()).and_then(|value| serde_json::from_value(value).ok()).ok_or_else(|| error("input", "Choose a Prism JSONL pack with a prismDictionary manifest, or provide source metadata for Wiktextract data."))?
                }
            };
            manifest
                .validate()
                .map_err(|message| error("input", &message))?;
            self.set_job(PackJob {
                phase: "importing".into(),
                language: Some(manifest.language.clone()),
                ..Default::default()
            });
            self.import_reader(reader, manifest)
        })();
        self.finish_job(&result);
        result
    }
    fn import_reader(&self, reader: impl BufRead, manifest: PackManifest) -> Result<InstalledPack> {
        self.store
            .import(reader, manifest, &self.cancel, |entries| {
                if let Ok(mut job) = self.job.lock() {
                    job.entries = entries;
                }
            })
            .map_err(|message| {
                error(
                    if self.cancel.load(Ordering::Acquire) {
                        "cancelled"
                    } else {
                        "storage"
                    },
                    &message,
                )
            })
    }
    pub fn download_pack(&self, language: &str) -> Result<InstalledPack> {
        let _operation = self
            .operation
            .try_lock()
            .map_err(|_| error("busy", "Another dictionary installation is running."))?;
        let name = match language {
            "ko" => "Korean",
            "en" => "English",
            "zh" => "Chinese",
            "ja" => "Japanese",
            _ => return Err(error("input", "Unsupported dictionary language.")),
        };
        self.cancel.store(false, Ordering::Release);
        self.set_job(PackJob {
            phase: "downloading".into(),
            language: Some(language.into()),
            ..Default::default()
        });
        let temporary = self.directory.join(format!("pack-{language}.download"));
        let result = (|| {
            // Only this fixed official corpus host is downloaded; a word is never part of the URL.
            let url =
                format!("https://kaikki.org/dictionary/{name}/kaikki.org-dictionary-{name}.jsonl");
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| {
                    error(
                        "network",
                        "Could not prepare the dictionary download worker.",
                    )
                })?;
            runtime.block_on(async {
            let client = reqwest::Client::builder()
                .https_only(true)
                .redirect(reqwest::redirect::Policy::none())
                .user_agent("Prism/0.1 (offline dictionary pack installation)")
                .connect_timeout(Duration::from_secs(8))
                .timeout(Duration::from_secs(900))
                .read_timeout(Duration::from_secs(3))
                .build()
                .map_err(|_| error("network", "Could not prepare the dictionary download."))?;
            let mut response = self.cancellable(client.get(url).send()).await?.map_err(|_| {
                error(
                    "network",
                    "Could not connect to the dictionary pack server.",
                )
            })?;
            if !response.status().is_success() {
                return Err(error(
                    "network",
                    "The dictionary pack server did not accept this download.",
                ));
            }
            let total = response.content_length();
            const MAX_DOWNLOAD: u64 = 4 * 1024 * 1024 * 1024;
            if total.is_some_and(|bytes| bytes > MAX_DOWNLOAD) {
                return Err(error("limit", "This pack exceeds the current 4 GiB download limit. Import an existing pack instead."));
            }
            if let Ok(mut job) = self.job.lock() {
                job.total_bytes = total;
            }
            let mut output = File::create(&temporary)
                .map_err(|_| error("storage", "Could not create the dictionary download file."))?;
            let mut bytes = 0u64;
            loop {
                if self.cancel.load(Ordering::Acquire) {
                    return Err(error("cancelled", "Dictionary installation cancelled."));
                }
                let chunk = self.cancellable(response.chunk()).await?.map_err(|_| error("network", "Dictionary download stopped or timed out. Retry when the connection is stable."))?;
                let Some(chunk) = chunk else { break; };
                bytes += chunk.len() as u64;
                if bytes > MAX_DOWNLOAD {
                    return Err(error("limit", "Dictionary download exceeds 4 GiB."));
                }
                output.write_all(&chunk).map_err(|_| {
                    error(
                        "storage",
                        "Dictionary download failed. Check free disk space.",
                    )
                })?;
                if let Ok(mut job) = self.job.lock() {
                    job.bytes = bytes;
                }
            }
            output
                .sync_all()
                .map_err(|_| error("storage", "Could not save the dictionary download."))?;
            drop(output);
            if total.is_some_and(|total| total != bytes) {
                return Err(error(
                    "network",
                    "Dictionary download was incomplete. The previous pack was preserved.",
                ));
            }
            Ok(())
            })?;
            let manifest = PackManifest {
                schema_version: 1,
                language: language.into(),
                name: format!("Kaikki / English Wiktionary — {name}"),
                version: chrono::Utc::now().to_rfc3339(),
                source_url: format!("https://kaikki.org/dictionary/{name}/"),
                license: "CC BY-SA 4.0 and GFDL".into(),
                license_url: "https://en.wiktionary.org/wiki/Wiktionary:Copyrights".into(),
                definition_language: "en".into(),
            };
            if let Ok(mut job) = self.job.lock() {
                job.phase = "importing".into();
            }
            let reader = BufReader::new(
                File::open(&temporary)
                    .map_err(|_| error("storage", "Could not reopen the dictionary download."))?,
            );
            self.import_reader(reader, manifest)
        })();
        let _ = fs::remove_file(temporary);
        self.finish_job(&result);
        result
    }
    async fn cancellable<F: std::future::Future>(&self, future: F) -> Result<F::Output> {
        let mut future = Box::pin(future);
        loop {
            if self.cancel.load(Ordering::Acquire) {
                return Err(error("cancelled", "Dictionary installation cancelled."));
            }
            if let Ok(result) = tokio::time::timeout(Duration::from_millis(100), &mut future).await
            {
                return Ok(result);
            }
        }
    }
    fn set_job(&self, job: PackJob) {
        if let Ok(mut slot) = self.job.lock() {
            *slot = job;
        }
    }
    fn finish_job(&self, result: &Result<InstalledPack>) {
        if let Ok(mut job) = self.job.lock() {
            match result {
                Ok(pack) => {
                    job.phase = "ready".into();
                    job.entries = pack.entries;
                    job.error = None;
                }
                Err(error) => {
                    job.phase = if error.code == "cancelled" {
                        "cancelled"
                    } else {
                        "error"
                    }
                    .into();
                    job.error = Some(error.clone());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancellation_drops_a_stalled_network_future_promptly() {
        let directory = tempfile::tempdir().unwrap();
        let service = DictionaryService::open(directory.path()).unwrap();
        let handle = service.clone();
        let cancel = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(40));
            handle.cancel();
        });
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let started = Instant::now();
        let result = runtime.block_on(service.cancellable(std::future::pending::<()>()));
        assert_eq!(result.unwrap_err().code, "cancelled");
        assert!(started.elapsed() < Duration::from_secs(1));
        cancel.join().unwrap();
    }
    #[test]
    fn offline_pack_import_restart_lookup_and_invalid_replacement() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("fixture.jsonl");
        let manifest = PackManifest {
            schema_version: 1,
            language: "ko".into(),
            name: "Synthetic fixture".into(),
            version: "1".into(),
            source_url: "https://example.org/fixture".into(),
            license: "CC0".into(),
            license_url: "https://creativecommons.org/publicdomain/zero/1.0/".into(),
            definition_language: "ko".into(),
        };
        fs::write(
            &source,
            format!(
                "{}\n{{\"term\":\"사과\",\"definitions\":[\"합성 테스트 뜻풀이\"]}}\n",
                serde_json::json!({"prismDictionary": manifest})
            ),
        )
        .unwrap();
        let service = DictionaryService::open(directory.path()).unwrap();
        let pack = service.import_pack(&source, None).unwrap();
        assert_eq!(service.job().unwrap().phase, "ready");
        let lookup = service.lookup("ko", "사과", true).unwrap();
        assert_eq!(lookup.source, "offline");
        assert_eq!(lookup.definitions[0].text, "합성 테스트 뜻풀이");
        assert!(lookup.offline_pack_installed);
        let network = service
            .lookup_with("ko", "사과", false, |_, _| {
                Err(ProviderError {
                    code: crate::dictionary_provider::ProviderErrorCode::Network,
                    message: "Synthetic network failure".into(),
                })
            })
            .unwrap();
        assert_eq!(network.source, "offline");
        assert_eq!(network.offline_reason.as_deref(), Some("network"));
        let no_word = service
            .lookup_with("ko", "사과", false, |_, _| {
                Err(ProviderError {
                    code: crate::dictionary_provider::ProviderErrorCode::NotFound,
                    message: "Synthetic missing entry".into(),
                })
            })
            .unwrap_err();
        assert_eq!(
            no_word.code, "notFound",
            "An online missing entry must not silently select an offline answer"
        );
        fs::write(&source, "invalid fixture").unwrap();
        assert!(service.import_pack(&source, Some(manifest)).is_err());
        drop(service);
        let service = DictionaryService::open(directory.path()).unwrap();
        assert_eq!(
            service
                .lookup("ko", "사과", true)
                .unwrap()
                .definitions
                .len(),
            1
        );
        assert!(
            !service
                .lookup("en", "apple", true)
                .unwrap()
                .offline_pack_installed
        );
        assert!(service.lookup("ko", "\u{0000}", true).is_err());
        service.remove(&pack.id).unwrap();
        assert!(service.packs().unwrap().is_empty());
    }
}
