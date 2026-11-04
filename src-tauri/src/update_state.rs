//! Small update bookkeeping, independent of editable settings revisions.
use crate::update_service::UpdateStatus;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Read,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct Record {
    pub last_check_ms: Option<u64>,
    pub dismissed_version: Option<String>,
}
pub struct UpdateState {
    path: PathBuf,
    saved: Mutex<Record>,
    failed: AtomicBool,
}
impl UpdateState {
    pub fn open(path: PathBuf) -> Result<Self, String> {
        let record = match fs::File::open(&path) {
            Ok(file) => {
                let mut bytes = vec![];
                file.take(65537)
                    .read_to_end(&mut bytes)
                    .map_err(|_| "Could not read update state.")?;
                if bytes.len() > 65536 {
                    return Err("Update state is too large; it was not changed.".into());
                }
                let record: Record = serde_json::from_slice(&bytes)
                    .map_err(|_| "Update state is damaged; it was not changed.")?;
                if record.dismissed_version.as_ref().is_some_and(|version| {
                    version.len() > 128 || semver::Version::parse(version).is_err()
                }) {
                    return Err("Update state has an invalid version; it was not changed.".into());
                }
                record
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Record::default(),
            Err(_) => return Err("Could not read update state.".into()),
        };
        Ok(Self {
            path,
            saved: Mutex::new(record),
            failed: AtomicBool::new(false),
        })
    }
    pub fn record(&self) -> Record {
        self.saved.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    pub fn persist(&self, status: &UpdateStatus) {
        let record = Record {
            last_check_ms: status.last_check_ms,
            dismissed_version: status.dismissed_version.clone(),
        };
        let mut saved = self.saved.lock().unwrap_or_else(|e| e.into_inner());
        if *saved == record {
            self.failed.store(false, Ordering::Release);
            return;
        }
        match crate::settings::write_json(&self.path, &record) {
            Ok(()) => {
                *saved = record;
                self.failed.store(false, Ordering::Release);
            }
            Err(_) => self.failed.store(true, Ordering::Release),
        }
    }
    pub fn failed(&self) -> bool {
        self.failed.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn metadata_survives_reopen_and_damage_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("updates/state.json");
        let store = UpdateState::open(path.clone()).unwrap();
        let mut status = UpdateStatus {
            last_check_ms: Some(42),
            dismissed_version: Some("0.2.0".into()),
            ..Default::default()
        };
        store.persist(&status);
        assert!(!store.failed());
        assert_eq!(
            UpdateState::open(path.clone())
                .unwrap()
                .record()
                .last_check_ms,
            Some(42)
        );
        let bytes = fs::read(&path).unwrap();
        status.checking = true;
        store.persist(&status);
        assert_eq!(
            fs::read(&path).unwrap(),
            bytes,
            "transient status does not rewrite metadata"
        );
        fs::write(&path, "broken").unwrap();
        assert!(UpdateState::open(path.clone()).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"broken");
    }
}
