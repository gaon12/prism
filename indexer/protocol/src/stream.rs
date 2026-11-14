//! Validate an entire reply, not just individual frames. A local drive letter
//! can refer to a per-user mapping: paths are accepted only after the service's
//! physical volume serial and kernel device match the caller's already-authorized root.
use crate::{
    AttributedFileRecord, DriveLetter, FailureCode, FileRecord, Request, Response,
    ValidatedResponse,
};

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
    AttributedRecords(Vec<AttributedFileRecord>),
    Removed(Vec<String>),
    Finished { revision: u64 },
}

pub struct Receiver {
    drive: DriveLetter,
    expected_serial: u64,
    expected_device: String,
    changes: bool,
    attributes: bool,
    announced: bool,
    terminal: bool,
    complete: bool,
    records: u64,
    path_bytes: u64,
}
impl Receiver {
    pub fn new(request: Request, expected_serial: u64, expected_device: &str) -> Self {
        Self {
            drive: request.drive(),
            expected_serial,
            expected_device: expected_device.into(),
            changes: matches!(request, Request::Changes { .. }),
            attributes: matches!(request, Request::SnapshotWithAttributes { .. }),
            announced: false,
            terminal: false,
            complete: false,
            records: 0,
            path_bytes: 0,
        }
    }
    fn account(
        &mut self,
        paths: impl Iterator<Item = impl AsRef<str>>,
        validated: Option<(u64, u64)>,
    ) -> Result<(), Error> {
        if let Some((records, bytes)) = validated {
            self.records = self.records.checked_add(records).ok_or(Error::Limit)?;
            self.path_bytes = self.path_bytes.checked_add(bytes).ok_or(Error::Limit)?;
            if self.records > MAX_RECORDS || self.path_bytes > MAX_PATH_BYTES {
                return Err(Error::Limit);
            }
            return Ok(());
        }
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
        self.accept_response(response, None)
    }
    /// Accept only this codec's owned validation proof for the requested drive.
    /// Constructed responses still use `accept`, including its path checks.
    pub fn accept_validated(
        &mut self,
        response: ValidatedResponse,
    ) -> Result<Option<Event>, Error> {
        if response.drive != self.drive {
            self.terminal = true;
            return Err(Error::Order);
        }
        self.accept_response(
            response.response,
            Some((response.path_count, response.path_bytes)),
        )
    }
    fn accept_response(
        &mut self,
        response: Response,
        validated: Option<(u64, u64)>,
    ) -> Result<Option<Event>, Error> {
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
                if !self.announced || self.attributes {
                    Err(Error::Order)
                } else {
                    self.account(records.iter().map(|record| record.path.as_str()), validated)?;
                    Ok(Some(Event::Records(records)))
                }
            }
            Response::AttributedRecords(records) => {
                if !self.announced || !self.attributes {
                    Err(Error::Order)
                } else {
                    self.account(
                        records.iter().map(|row| row.record.path.as_str()),
                        validated,
                    )?;
                    Ok(Some(Event::AttributedRecords(records)))
                }
            }
            Response::Removed(paths) => {
                if !self.announced || !self.changes {
                    Err(Error::Order)
                } else {
                    self.account(paths.iter(), validated)?;
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
    #[test]
    fn attribute_rows_require_opt_in_identity_order_and_the_same_budgets() {
        let drive = DriveLetter::new(b'C').unwrap();
        let attributed = || {
            Response::AttributedRecords(vec![AttributedFileRecord {
                record: record(),
                attributes: Some(0x8000_0026),
            }])
        };
        let metadata = Request::SnapshotWithAttributes { drive };
        for checked in [false, true] {
            let mut receiver = Receiver::new(metadata, 7, DEVICE);
            receiver.accept(volume(7)).unwrap();
            let event = if checked {
                receiver.accept_validated(decoded(&attributed(), drive))
            } else {
                receiver.accept(attributed())
            }
            .unwrap();
            assert!(
                matches!(event, Some(Event::AttributedRecords(rows)) if rows[0].attributes == Some(0x8000_0026) && rows[0].record == record())
            );
            receiver.accept(Response::Finished { revision: 4 }).unwrap();
            assert!(receiver.finished());
        }
        for (request, response, announce) in [
            (request(), attributed(), true),
            (metadata, Response::Records(vec![record()]), true),
            (metadata, attributed(), false),
        ] {
            let mut receiver = Receiver::new(request, 7, DEVICE);
            if announce {
                receiver.accept(volume(7)).unwrap();
            }
            assert_eq!(receiver.accept(response), Err(Error::Order));
            assert!(!receiver.finished());
            assert_eq!(
                receiver.accept(Response::Finished { revision: 4 }),
                Err(Error::Order)
            );
        }
        for (records, bytes) in [(MAX_RECORDS, 0), (0, MAX_PATH_BYTES)] {
            let mut receiver = Receiver::new(metadata, 7, DEVICE);
            receiver.accept(volume(7)).unwrap();
            receiver.records = records;
            receiver.path_bytes = bytes;
            assert_eq!(
                receiver.accept_validated(decoded(&attributed(), drive)),
                Err(Error::Limit)
            );
        }
        let mut receiver = Receiver::new(metadata, 7, DEVICE);
        assert_eq!(receiver.accept(volume(8)), Err(Error::VolumeMismatch));
        assert_eq!(receiver.accept(attributed()), Err(Error::Order));
    }

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
    fn decoded(response: &Response, drive: DriveLetter) -> ValidatedResponse {
        let mut bytes = Vec::new();
        response.write(&mut bytes, drive).unwrap();
        Response::read_validated(&mut std::io::Cursor::new(bytes), drive).unwrap()
    }
    #[test]
    fn codec_validated_and_constructed_streams_preserve_the_same_events_and_metadata() {
        for request in [
            request(),
            Request::Changes {
                drive: DriveLetter::new(b'C').unwrap(),
                after: 1,
            },
        ] {
            let mut ordinary = Receiver::new(request, 7, DEVICE);
            let mut validated = Receiver::new(request, 7, DEVICE);
            let mut events = vec![
                Response::Progress { completed: 8 },
                volume(7),
                Response::Records(vec![
                    record(),
                    FileRecord {
                        path: "C:\\문서\\가 😀.txt".into(),
                        is_dir: true,
                        size: u64::MAX,
                        modified_ms: None,
                    },
                ]),
            ];
            if matches!(request, Request::Changes { .. }) {
                events.push(Response::Removed(vec!["C:\\old.md".into()]));
            }
            events.push(Response::Finished { revision: 2 });
            for response in events {
                let checked = decoded(&response, ordinary.drive);
                assert_eq!(
                    ordinary.accept(response),
                    validated.accept_validated(checked)
                );
                assert_eq!(ordinary.finished(), validated.finished());
                assert_eq!(ordinary.records, validated.records);
                assert_eq!(ordinary.path_bytes, validated.path_bytes);
            }
            assert!(validated.finished());
        }
    }

    #[test]
    fn codec_validation_cannot_replace_drive_volume_or_order_checks() {
        let drive = DriveLetter::new(b'C').unwrap();
        let mut receiver = Receiver::new(request(), 7, DEVICE);
        assert_eq!(
            receiver.accept_validated(decoded(&Response::Records(vec![record()]), drive)),
            Err(Error::Order)
        );
        assert_eq!(
            receiver.accept_validated(decoded(&volume(7), drive)),
            Err(Error::Order)
        );

        let mut receiver = Receiver::new(request(), 7, DEVICE);
        assert_eq!(
            receiver.accept_validated(decoded(&volume(8), drive)),
            Err(Error::VolumeMismatch)
        );
        assert!(!receiver.finished());

        let mut receiver = Receiver::new(request(), 7, DEVICE);
        receiver.accept(volume(7)).unwrap();
        let other = DriveLetter::new(b'D').unwrap();
        let wrong_drive = Response::Records(vec![FileRecord {
            path: "D:\\same-volume-mapping.md".into(),
            ..record()
        }]);
        assert_eq!(
            receiver.accept_validated(decoded(&wrong_drive, other)),
            Err(Error::Order)
        );
        assert!(!receiver.finished());

        // The public constructed-response API must still validate every path.
        let mut receiver = Receiver::new(request(), 7, DEVICE);
        receiver.accept(volume(7)).unwrap();
        assert_eq!(receiver.accept(wrong_drive), Err(Error::Order));
    }

    #[test]
    fn validated_batches_keep_exact_global_budgets_and_terminal_failure() {
        let drive = DriveLetter::new(b'C').unwrap();
        let response = Response::Records(vec![record()]);
        let bytes = record().path.len() as u64;
        for (records, path_bytes, expected) in [
            (MAX_RECORDS - 1, MAX_PATH_BYTES - bytes, true),
            (MAX_RECORDS, 0, false),
            (0, MAX_PATH_BYTES - bytes + 1, false),
            (u64::MAX, 0, false),
            (0, u64::MAX, false),
        ] {
            for fast in [false, true] {
                let mut receiver = Receiver::new(request(), 7, DEVICE);
                receiver.accept(volume(7)).unwrap();
                receiver.records = records;
                receiver.path_bytes = path_bytes;
                let result = if fast {
                    receiver.accept_validated(decoded(&response, drive))
                } else {
                    receiver.accept(response.clone())
                };
                if expected {
                    assert!(matches!(result, Ok(Some(Event::Records(_)))));
                    assert_eq!(receiver.records, MAX_RECORDS);
                    assert_eq!(receiver.path_bytes, MAX_PATH_BYTES);
                    assert!(receiver.accept(Response::Finished { revision: 1 }).is_ok());
                } else {
                    assert_eq!(result, Err(Error::Limit));
                    assert_eq!(
                        receiver.accept(Response::Finished { revision: 1 }),
                        Err(Error::Order)
                    );
                    assert!(!receiver.finished());
                }
            }
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
