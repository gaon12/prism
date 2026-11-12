//! Persist the merged checkpoint without reconstructing a full catalog.
use super::*;

impl<R: Read + Seek> Reader<R> {
    /// Consume the immutable source while writing only to a new empty stage.
    /// On return, including failure, the reader and its input are dropped.
    /// Native callers must flush/fsync the stage and only then atomically
    /// publish it; this function never replaces or truncates the source file.
    pub fn write(
        mut self,
        device: &str,
        out: &mut (impl Write + Seek),
        check: impl Fn() -> super::super::super::layout::Result<()>,
    ) -> io::Result<()> {
        write_using(
            (self.geometry(), self.checkpoint(), self.record_count()),
            device,
            (&self.directories.owners, &self.directories.families),
            out,
            &check,
            |accept| {
                visit_merged(
                    &mut self.input,
                    &self.references,
                    self.records_start,
                    self.records_bytes,
                    &self.overrides,
                    &check,
                    |record| accept(&record),
                )
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::tests::{encoded, sample, DEVICE};
    use super::*;
    use std::{cell::Cell, io::Cursor, rc::Rc};

    struct TrackedInput {
        input: Cursor<Vec<u8>>,
        closed: Rc<Cell<bool>>,
    }
    impl Read for TrackedInput {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            self.input.read(bytes)
        }
    }
    impl Seek for TrackedInput {
        fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
            self.input.seek(position)
        }
    }
    impl Drop for TrackedInput {
        fn drop(&mut self) {
            self.closed.set(true);
        }
    }
    fn reader(bytes: Vec<u8>, closed: Rc<Cell<bool>>) -> Reader<TrackedInput> {
        Reader::open(
            TrackedInput {
                input: Cursor::new(bytes),
                closed,
            },
            sample().geometry,
            DEVICE,
            || Ok(()),
        )
        .unwrap()
    }
    #[test]
    fn writing_consumes_the_input_on_success_cancellation_and_rejected_destination() {
        let bytes = encoded(&sample());
        let closed = Rc::new(Cell::new(false));
        let mut output = Cursor::new(Vec::new());
        reader(bytes.clone(), closed.clone())
            .write(DEVICE, &mut output, || Ok(()))
            .unwrap();
        assert!(closed.get());
        assert_eq!(output.into_inner(), bytes);
        let closed = Rc::new(Cell::new(false));
        let mut output = Cursor::new(b"preserved destination".to_vec());
        assert!(reader(bytes.clone(), closed.clone())
            .write(DEVICE, &mut output, || Ok(()))
            .is_err());
        assert!(closed.get());
        assert_eq!(output.into_inner(), b"preserved destination");
        let closed = Rc::new(Cell::new(false));
        let mut output = Cursor::new(Vec::new());
        let calls = Cell::new(0);
        let result = reader(bytes, closed.clone()).write(DEVICE, &mut output, || {
            let n = calls.get();
            calls.set(n + 1);
            if n >= 4 {
                Err(crate::layout::Error::Cancelled)
            } else {
                Ok(())
            }
        });
        assert!(result.is_err());
        assert!(closed.get());
        assert!(
            output.get_ref().len() > HEADER as usize,
            "cancellation after a private partial write"
        );
        assert!(
            !output.get_ref().starts_with(MAGIC),
            "partial stage must not have a completed header"
        );
    }

    struct FullDisk(Cursor<Vec<u8>>);
    impl Write for FullDisk {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.0.position() + bytes.len() as u64 > 128 {
                return Err(io::ErrorKind::StorageFull.into());
            }
            self.0.write(bytes)
        }
        fn flush(&mut self) -> io::Result<()> {
            self.0.flush()
        }
    }
    impl Seek for FullDisk {
        fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
            self.0.seek(position)
        }
    }
    #[test]
    fn staging_io_failure_releases_the_input_without_finalizing_the_stage() {
        let closed = Rc::new(Cell::new(false));
        let mut output = FullDisk(Cursor::new(Vec::new()));
        let result =
            reader(encoded(&sample()), closed.clone()).write(DEVICE, &mut output, || Ok(()));
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::StorageFull);
        assert!(closed.get());
        assert!(!output.0.get_ref().starts_with(MAGIC));
    }
}
