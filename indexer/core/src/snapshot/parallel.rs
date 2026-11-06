//! Bounded decoding pipeline. Source I/O, cancellation/progress callbacks,
//! catalog mutation and journal replay stay on the calling thread. A worker
//! owns at most one raw batch; results are merged in original MFT order.
use super::*;
use crate::MetadataRecord;
use std::{
    io,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender},
    },
    thread,
    time::Duration,
};

struct Job {
    first: u64,
    bytes: Vec<u8>,
}
struct Decoded {
    records: Vec<MetadataRecord>,
    repair: Vec<u64>,
    bytes: Vec<u8>,
}
struct Worker {
    input: SyncSender<Job>,
    output: Receiver<layout::Result<Decoded>>,
    pending: bool,
}
fn worker_error() -> Error {
    Error::Io(io::ErrorKind::Other)
}

fn decode(job: Job, geometry: Geometry, stopped: &AtomicBool) -> layout::Result<Decoded> {
    let mut result = Decoded {
        records: Vec::new(),
        repair: Vec::new(),
        bytes: Vec::new(),
    };
    for (index, bytes) in job
        .bytes
        .chunks_exact(geometry.record_size as usize)
        .enumerate()
    {
        if index % 256 == 0 && stopped.load(Ordering::Acquire) {
            return Err(Error::Cancelled);
        }
        let number = job.first + index as u64;
        match parse_record(number, geometry.sector_size as usize, bytes) {
            Ok(Some(record)) => result.records.push(record),
            Ok(None) => {}
            Err(_) => {
                result.repair.push(number);
                if result.repair.len() > 4096 {
                    return Err(Error::Limit);
                }
            }
        }
    }
    result.bytes = job.bytes;
    Ok(result)
}

struct Pipeline<'a> {
    workers: Vec<Worker>,
    next: usize,
    next_number: u64,
    record_bytes: usize,
    batch_bytes: usize,
    builder: &'a mut CatalogBuilder,
    repair: &'a mut BTreeSet<u64>,
    check: &'a dyn Fn() -> layout::Result<()>,
}
impl Pipeline<'_> {
    fn receive(&mut self, slot: usize) -> layout::Result<Vec<u8>> {
        let decoded = loop {
            (self.check)()?;
            match self.workers[slot]
                .output
                .recv_timeout(Duration::from_millis(10))
            {
                Ok(result) => break result?,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => return Err(worker_error()),
            }
        };
        self.workers[slot].pending = false;
        for (index, record) in decoded.records.into_iter().enumerate() {
            if index % 256 == 0 {
                (self.check)()?;
            }
            self.builder.insert(record)?;
        }
        for number in decoded.repair {
            self.repair.insert(number);
            if self.repair.len() > 4096 {
                return Err(Error::Limit);
            }
        }
        Ok(decoded.bytes)
    }
    fn submit(&mut self, first: u64, bytes: Vec<u8>) -> layout::Result<Vec<u8>> {
        if bytes.is_empty()
            || bytes.len() > self.batch_bytes
            || !bytes.len().is_multiple_of(self.record_bytes)
            || first != self.next_number
        {
            return Err(Error::Corrupt);
        }
        self.next_number = first
            .checked_add((bytes.len() / self.record_bytes) as u64)
            .ok_or(Error::Limit)?;
        (self.check)()?;
        let slot = self.next;
        let recycled = if self.workers[slot].pending {
            self.receive(slot)?
        } else {
            Vec::with_capacity(self.batch_bytes)
        };
        // There is only one outstanding job per worker. This channel must be
        // empty after receiving the previous result; never block on a send.
        self.workers[slot]
            .input
            .try_send(Job { first, bytes })
            .map_err(|_| worker_error())?;
        self.workers[slot].pending = true;
        self.next = (slot + 1) % self.workers.len();
        Ok(recycled)
    }
    fn finish(&mut self) -> layout::Result<()> {
        // The ring cursor is the oldest outstanding batch. Ignore unused
        // slots for a short final batch, while retaining deterministic order.
        for offset in 0..self.workers.len() {
            let slot = (self.next + offset) % self.workers.len();
            if self.workers[slot].pending {
                let _ = self.receive(slot)?;
            }
        }
        (self.check)()
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn scan(
    layout: &MftLayout,
    source: &mut impl layout::Source,
    geometry: Geometry,
    builder: &mut CatalogBuilder,
    repair: &mut BTreeSet<u64>,
    check: &impl Fn() -> layout::Result<()>,
    mut progress: impl FnMut(u64),
    workers: usize,
    batch_bytes: usize,
) -> layout::Result<()> {
    run(
        geometry,
        builder,
        repair,
        check,
        workers,
        batch_bytes,
        |pipeline| {
            // Move each owned read buffer, then reuse an older completed one.
            // Source/check/progress still never cross a thread boundary.
            layout.scan_owned_batches(
                source,
                batch_bytes,
                |number, bytes| pipeline.submit(number, bytes),
                |processed| {
                    check()?;
                    progress(processed);
                    Ok(())
                },
            )
        },
    )
}

fn run(
    geometry: Geometry,
    builder: &mut CatalogBuilder,
    repair: &mut BTreeSet<u64>,
    check: &impl Fn() -> layout::Result<()>,
    workers: usize,
    batch_bytes: usize,
    feed: impl FnOnce(&mut Pipeline<'_>) -> layout::Result<()>,
) -> layout::Result<()> {
    geometry.validate()?;
    let record_bytes = geometry.record_size as usize;
    if !(2..=8).contains(&workers)
        || !(record_bytes..=4 * 1024 * 1024).contains(&batch_bytes)
        || !batch_bytes.is_multiple_of(record_bytes)
    {
        return Err(Error::Limit);
    }
    let stopped = AtomicBool::new(false);
    thread::scope(|scope| {
        let mut handles = Vec::new();
        let mut pipeline = Pipeline {
            workers: Vec::new(),
            next: 0,
            next_number: 0,
            record_bytes,
            batch_bytes,
            builder,
            repair,
            check,
        };
        let result = (|| {
            for _ in 0..workers {
                let (input, jobs) = mpsc::sync_channel::<Job>(1);
                let (results, output) = mpsc::sync_channel(1);
                let stopped = &stopped;
                let handle = thread::Builder::new()
                    .name("prism-mft-parse".into())
                    .spawn_scoped(scope, move || {
                        for job in jobs {
                            if stopped.load(Ordering::Acquire) {
                                break;
                            }
                            let decoded = decode(job, geometry, stopped);
                            if results.send(decoded).is_err() {
                                break;
                            }
                        }
                    })
                    .map_err(|error| Error::Io(error.kind()))?;
                handles.push(handle);
                pipeline.workers.push(Worker {
                    input,
                    output,
                    pending: false,
                });
            }
            feed(&mut pipeline)?;
            pipeline.finish()
        })();
        // On every exit disconnect both directions before joining: a worker
        // might be waiting for input or trying to return a completed batch.
        // No new request may pretend these real workers have already ended.
        stopped.store(true, Ordering::Release);
        drop(pipeline);
        let mut joined = Ok(());
        for handle in handles {
            if handle.join().is_err() {
                joined = Err(worker_error());
            }
        }
        result.and(joined)
    })
}

#[cfg(test)]
#[path = "parallel_tests.rs"]
mod tests;
