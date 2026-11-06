use super::*;
use flate2::read::GzDecoder;
use std::{cell::Cell, io::Read};

pub(super) fn geometry() -> Geometry {
    Geometry {
        sector_size: 512,
        cluster_size: 512,
        record_size: 1024,
        volume_bytes: 2097152,
        mft_bytes: 594944,
        serial: 1,
    }
}
pub(super) fn public_mft() -> Vec<u8> {
    let mut bytes = Vec::new();
    GzDecoder::new(&include_bytes!("../../tests/fixtures/testfs1-mft.gz")[..])
        .read_to_end(&mut bytes)
        .unwrap();
    bytes
}
// A deterministic memory source for decode/merge checks. Both sequential
// and parallel timings perform this same simulated read into reusable
// owned buffers; no extra per-record packing copy is hidden in one side.
pub(super) fn produce(
    count: u64,
    payload: &[u8],
    batch_bytes: usize,
    mut receive: impl FnMut(u64, Vec<u8>) -> layout::Result<Vec<u8>>,
) -> layout::Result<()> {
    let mut buffer = Vec::with_capacity(batch_bytes);
    let mut first = 0;
    while first < count {
        let length = (count - first).min((batch_bytes / 1024) as u64) as usize * 1024;
        buffer.resize(length, 0);
        for (index, output) in buffer.as_chunks_mut::<1024>().0.iter_mut().enumerate() {
            let offset = (first + index as u64) as usize % (payload.len() / 1024) * 1024;
            output.copy_from_slice(&payload[offset..offset + 1024]);
        }
        buffer = receive(first, buffer)?;
        first += (length / 1024) as u64;
    }
    Ok(())
}
fn records(builder: CatalogBuilder) -> Vec<prism_ntfs_protocol::FileRecord> {
    let mut result = Vec::new();
    builder
        .finish()
        .unwrap()
        .visit(
            prism_ntfs_protocol::DriveLetter::new(b'C').unwrap(),
            |record| {
                result.push(record);
                Ok::<_, ()>(())
            },
        )
        .unwrap();
    result
}

#[test]
fn many_bounded_jobs_match_the_complete_independent_image() {
    let bytes = public_mft();
    let mut sequential = CatalogBuilder::default();
    for (number, raw) in bytes.as_chunks::<1024>().0.iter().enumerate() {
        if let Some(record) = parse_record(number as u64, 512, raw).unwrap() {
            sequential.insert(record).unwrap();
        }
    }
    let expected = records(sequential);
    for workers in [2, 4, 8] {
        let mut actual = CatalogBuilder::default();
        let mut repair = BTreeSet::new();
        let mut buffers = BTreeSet::new();
        run(
            geometry(),
            &mut actual,
            &mut repair,
            &|| Ok(()),
            workers,
            32 * 1024,
            |pipeline| {
                produce(581, &bytes, 32 * 1024, |first, bytes| {
                    buffers.insert(bytes.as_ptr() as usize);
                    pipeline.submit(first, bytes)
                })
            },
        )
        .unwrap();
        assert!(repair.is_empty());
        assert_eq!(
            buffers.len(),
            workers + 1,
            "raw buffers are recycled, not copied or accumulated"
        );
        assert_eq!(records(actual), expected);
    }
    assert_eq!(
        expected
            .iter()
            .filter(|file| file.path.starts_with("C:\\many_subdirs\\"))
            .count(),
        512
    );
}

#[test]
fn repair_budget_is_global_across_workers_and_local_batches_are_bounded() {
    let mut builder = CatalogBuilder::default();
    let mut repair = BTreeSet::new();
    assert_eq!(
        run(
            geometry(),
            &mut builder,
            &mut repair,
            &|| Ok(()),
            4,
            128 * 1024,
            |pipeline| produce(5000, &[0xff; 1024], 128 * 1024, |first, bytes| pipeline
                .submit(first, bytes))
        ),
        Err(Error::Limit)
    );
    assert_eq!(repair.len(), 4097);
    assert!(builder.records.is_empty());
    let mut small_records = geometry();
    small_records.record_size = 512;
    assert!(matches!(
        decode(
            Job {
                first: 0,
                bytes: vec![0xff; 4 * 1024 * 1024]
            },
            small_records,
            &AtomicBool::new(false)
        ),
        Err(Error::Limit)
    ));
    for (workers, bytes) in [(0, 1024), (9, 1024), (2, 1023), (2, 4 * 1024 * 1024 + 1024)] {
        assert_eq!(
            run(
                geometry(),
                &mut builder,
                &mut repair,
                &|| Ok(()),
                workers,
                bytes,
                |_| panic!("invalid pool must not call producer")
            ),
            Err(Error::Limit)
        );
    }
}

#[test]
fn cancellation_deadlines_and_producer_failures_join_before_returning() {
    let raw = public_mft()[65 * 1024..66 * 1024].to_vec();
    for failure in [
        Error::Cancelled,
        Error::TimedOut,
        Error::Io(io::ErrorKind::UnexpectedEof),
    ] {
        for _ in 0..8 {
            let stopped = Cell::new(false);
            let coordinator = thread::current().id();
            let check = || {
                assert_eq!(thread::current().id(), coordinator);
                if stopped.get() {
                    Err(failure)
                } else {
                    Ok(())
                }
            };
            let mut builder = CatalogBuilder::default();
            let mut repair = BTreeSet::new();
            assert_eq!(
                run(
                    geometry(),
                    &mut builder,
                    &mut repair,
                    &check,
                    4,
                    8 * 1024,
                    |pipeline| {
                        produce(32, &raw, 8 * 1024, |first, bytes| {
                            pipeline.submit(first, bytes)
                        })?;
                        // Four real jobs are outstanding. Returning an error must
                        // disconnect pending input/results and join those workers.
                        stopped.set(true);
                        Err(failure)
                    }
                ),
                Err(failure)
            );
        }
    }
}
