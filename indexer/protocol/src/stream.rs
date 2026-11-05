//! Validate an entire reply, not just individual frames. A local drive letter
//! can refer to a per-user mapping: paths are accepted only after the service's
//! physical volume serial and kernel device match the caller's already-authorized root.
use crate::{DriveLetter, FailureCode, FileRecord, Request, Response};

const MAX_RECORDS: u64 = 10_000_000;
const MAX_PATH_BYTES: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Order,
    VolumeMismatch,
    Limit,
    Service(FailureCode),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Filename index stream rejected ({self:?}).")
    }
}
impl std::error::Error for Error {}

#[derive(Debug, Eq, PartialEq)]
pub enum Event {
    Progress { completed: u64 },
    Records(Vec<FileRecord>),
    Removed(Vec<String>),
    Finished { revision: u64 },
}

pub struct Receiver {
    drive: DriveLetter,
    expected_serial: u64,
    expected_device: String,
    changes: bool,
    announced: bool,
    terminal: bool,
    complete: bool,
    records: u64,
    path_bytes: u64,
}
impl Receiver {
    pub fn new(request: Request, expected_serial: u64, expected_device: &str) -> Self {
        let (drive, changes) = match request {
            Request::Snapshot { drive } => (drive, false),
            Request::Changes { drive, .. } => (drive, true),
        };
        Self {
            drive,
            expected_serial,
            expected_device: expected_device.into(),
            changes,
            announced: false,
            terminal: false,
            complete: false,
            records: 0,
            path_bytes: 0,
        }
    }
    fn account(&mut self, paths: impl Iterator<Item = impl AsRef<str>>) -> Result<(), Error> {
        for path in paths {
            let path = path.as_ref();
            if !self.drive.contains(path) {
                return Err(Error::Order);
            }
            self.records = self.records.checked_add(1).ok_or(Error::Limit)?;
            self.path_bytes = self
                .path_bytes
                .checked_add(path.len() as u64)
                .ok_or(Error::Limit)?;
            if self.records > MAX_RECORDS || self.path_bytes > MAX_PATH_BYTES {
                return Err(Error::Limit);
            }
        }
        Ok(())
    }
    pub fn accept(&mut self, response: Response) -> Result<Option<Event>, Error> {
        if self.terminal {
            return Err(Error::Order);
        }
        let result = (|| match response {
            Response::Volume { serial, device } => {
                if self.announced {
                    Err(Error::Order)
                } else if serial != self.expected_serial
                    || !device.eq_ignore_ascii_case(&self.expected_device)
                {
                    Err(Error::VolumeMismatch)
                } else {
                    self.announced = true;
                    Ok(None)
                }
            }
            Response::Progress { completed } => Ok(Some(Event::Progress { completed })),
            Response::Records(records) => {
                if !self.announced {
                    Err(Error::Order)
                } else {
                    self.account(records.iter().map(|record| record.path.as_str()))?;
                    Ok(Some(Event::Records(records)))
                }
            }
            Response::Removed(paths) => {
                if !self.announced || !self.changes {
                    Err(Error::Order)
                } else {
                    self.account(paths.iter())?;
                    Ok(Some(Event::Removed(paths)))
                }
            }
            Response::Finished { revision } => {
                if !self.announced || revision == 0 {
                    Err(Error::Order)
                } else {
                    self.terminal = true;
                    self.complete = true;
                    Ok(Some(Event::Finished { revision }))
                }
            }
            Response::Failure(code) => Err(Error::Service(code)),
        })();
        if result.is_err() {
            self.terminal = true;
        }
        result
    }
    pub fn finished(&self) -> bool {
        self.complete
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const DEVICE: &str = r"\Device\TestVolume";
    fn volume(serial: u64) -> Response {
        Response::Volume {
            serial,
            device: DEVICE.into(),
        }
    }
    fn request() -> Request {
        Request::Snapshot {
            drive: DriveLetter::new(b'C').unwrap(),
        }
    }
    fn record() -> FileRecord {
        FileRecord {
            path: "C:\\문서.md".into(),
            is_dir: false,
            size: 42,
            modified_ms: Some(17),
        }
    }
    #[test]
    fn identity_precedes_paths_and_only_a_finished_stream_is_complete() {
        let mut receiver = Receiver::new(request(), 7, DEVICE);
        assert!(!receiver.finished());
        assert_eq!(
            receiver
                .accept(Response::Progress { completed: 128 })
                .unwrap(),
            Some(Event::Progress { completed: 128 })
        );
        receiver.accept(volume(7)).unwrap();
        assert_eq!(
            receiver.accept(Response::Records(vec![record()])).unwrap(),
            Some(Event::Records(vec![record()]))
        );
        assert!(!receiver.finished());
        assert_eq!(
            receiver.accept(Response::Finished { revision: 1 }).unwrap(),
            Some(Event::Finished { revision: 1 })
        );
        assert!(receiver.finished());
        assert_eq!(
            receiver
                .accept(Response::Progress { completed: 999 })
                .unwrap_err(),
            Error::Order
        );
    }
    #[test]
    fn reject_forged_mapping_early_paths_duplicate_headers_and_partial_completion() {
        let mut receiver = Receiver::new(request(), 7, DEVICE);
        assert_eq!(
            receiver.accept(volume(8)).unwrap_err(),
            Error::VolumeMismatch
        );
        assert_eq!(receiver.accept(volume(7)).unwrap_err(), Error::Order);
        let mut receiver = Receiver::new(request(), 7, DEVICE);
        assert_eq!(
            receiver
                .accept(Response::Records(vec![record()]))
                .unwrap_err(),
            Error::Order
        );
        let mut receiver = Receiver::new(request(), 7, DEVICE);
        assert_eq!(
            receiver
                .accept(Response::Finished { revision: 1 })
                .unwrap_err(),
            Error::Order
        );
        let mut receiver = Receiver::new(request(), 7, DEVICE);
        receiver.accept(volume(7)).unwrap();
        assert_eq!(receiver.accept(volume(7)).unwrap_err(), Error::Order);
    }
    #[test]
    fn cloned_serial_on_a_different_kernel_device_is_rejected() {
        let mut receiver = Receiver::new(request(), 7, DEVICE);
        assert_eq!(
            receiver
                .accept(Response::Volume {
                    serial: 7,
                    device: r"\Device\DifferentVolume".into()
                })
                .unwrap_err(),
            Error::VolumeMismatch
        );
        assert!(!receiver.finished());
        let mut receiver = Receiver::new(request(), 7, DEVICE);
        receiver
            .accept(Response::Volume {
                serial: 7,
                device: DEVICE.to_ascii_lowercase(),
            })
            .unwrap();
        assert!(receiver.accept(Response::Finished { revision: 1 }).is_ok());
    }
    #[test]
    fn snapshots_reject_deletions_and_changes_retain_explicit_failure() {
        let mut receiver = Receiver::new(request(), 7, DEVICE);
        receiver.accept(volume(7)).unwrap();
        assert_eq!(
            receiver
                .accept(Response::Removed(vec!["C:\\old.md".into()]))
                .unwrap_err(),
            Error::Order
        );
        let mut receiver = Receiver::new(
            Request::Changes {
                drive: DriveLetter::new(b'C').unwrap(),
                after: 1,
            },
            7,
            DEVICE,
        );
        receiver.accept(volume(7)).unwrap();
        assert_eq!(
            receiver
                .accept(Response::Removed(vec!["C:\\old.md".into()]))
                .unwrap(),
            Some(Event::Removed(vec!["C:\\old.md".into()]))
        );
        assert_eq!(
            receiver
                .accept(Response::Failure(FailureCode::JournalGap))
                .unwrap_err(),
            Error::Service(FailureCode::JournalGap)
        );
        assert!(!receiver.finished());
        assert_eq!(
            receiver
                .accept(Response::Finished { revision: 2 })
                .unwrap_err(),
            Error::Order
        );
    }
    #[test]
    fn apply_total_stream_budgets_as_well_as_per_frame_budgets() {
        let mut receiver = Receiver::new(request(), 7, DEVICE);
        receiver.accept(volume(7)).unwrap();
        receiver.records = MAX_RECORDS;
        assert_eq!(
            receiver
                .accept(Response::Records(vec![record()]))
                .unwrap_err(),
            Error::Limit
        );
        assert!(!receiver.finished());
        assert_eq!(
            receiver
                .accept(Response::Finished { revision: 1 })
                .unwrap_err(),
            Error::Order
        );
        let mut receiver = Receiver::new(request(), 7, DEVICE);
        receiver.accept(volume(7)).unwrap();
        receiver.path_bytes = MAX_PATH_BYTES;
        assert_eq!(
            receiver
                .accept(Response::Records(vec![record()]))
                .unwrap_err(),
            Error::Limit
        );
    }
}
