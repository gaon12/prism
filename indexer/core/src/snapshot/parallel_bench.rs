//! Synthetic decode/merge benchmark. No live filesystem, service or cold-start claim.
use super::tests::{geometry, produce, public_mft};
use super::*;
use std::time::Instant;

#[test]
#[ignore = "synthetic decode/merge timing; not live C-drive or service performance"]
fn large_metadata_decode_and_merge_timing() {
    let bytes = public_mft();
    let count = 1_000_000u64;
    for (workload, payload) in [
        ("resident", &bytes[65 * 1024..66 * 1024]),
        ("public-cycle", bytes.as_slice()),
    ] {
        let mut expected = None;
        for workers in [1, 2, 4, 8] {
            let mut times = Vec::new();
            for _ in 0..3 {
                let mut builder = CatalogBuilder::default();
                let mut repair = BTreeSet::new();
                let start = Instant::now();
                if workers == 1 {
                    produce(count, payload, 4 * 1024 * 1024, |first, bytes| {
                        for (index, raw) in bytes.as_chunks::<1024>().0.iter().enumerate() {
                            if let Some(record) =
                                parse_record(first + index as u64, 512, raw).unwrap()
                            {
                                builder.insert(record).unwrap();
                            }
                        }
                        Ok(bytes)
                    })
                    .unwrap();
                } else {
                    run(
                        geometry(),
                        &mut builder,
                        &mut repair,
                        &|| Ok(()),
                        workers,
                        4 * 1024 * 1024,
                        |pipeline| {
                            produce(count, payload, 4 * 1024 * 1024, |first, bytes| {
                                pipeline.submit(first, bytes)
                            })
                        },
                    )
                    .unwrap();
                }
                times.push(start.elapsed().as_millis());
                let sum: u64 = builder
                    .records
                    .values()
                    .map(|record| record.reference.number() + record.size.unwrap_or(0))
                    .sum();
                assert!(repair.is_empty());
                let actual = (builder.records.len(), sum);
                if let Some(expected) = expected {
                    assert_eq!(actual, expected);
                } else {
                    expected = Some(actual);
                }
            }
            times.sort_unstable();
            println!("{{\"synthetic\":true,\"workload\":\"{workload}\",\"inputRecords\":{count},\"workers\":{workers},\"medianDecodeMergeMs\":{},\"samples\":{:?}}}", times[1], times);
        }
    }
}
