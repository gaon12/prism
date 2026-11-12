//! A snapshot is published only after replaying changes made during the bulk
//! MFT read. Work-in-progress builders never escape on cancellation or errors.
use crate::{
    journal,
    layout::{self, Error, Geometry, MftLayout},
    parse_record, Catalog, CatalogBuilder, FileReference,
};
use std::collections::BTreeSet;
#[cfg(feature = "cache")]
pub mod codec;
mod parallel;
#[cfg(test)]
#[path = "snapshot/parallel_snapshot_tests.rs"]
mod parallel_snapshot_tests;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Progress {
    Mft { processed: u64, total: u64 },
    Replay { processed: u64, total: u64 },
}
pub struct Snapshot {
    pub catalog: Catalog,
    pub checkpoint: journal::Checkpoint,
    pub records_read: u64,
    pub geometry: Geometry,
}

pub fn collect(
    source: &mut (impl layout::Source + journal::Source),
    geometry: Geometry,
    check: impl Fn() -> layout::Result<()>,
    progress: impl FnMut(Progress),
) -> layout::Result<Snapshot> {
    // A small MFT cannot amortize a worker pool. Keep its existing sequential
    // path; only pure decoding, never the volume/journal source, is parallel.
    let workers = if geometry.mft_bytes >= 16 * 1024 * 1024 {
        std::thread::available_parallelism()
            .map_or(1, usize::from)
            // Four decoders matched or beat eight in the owned-buffer
            // benchmark, without doubling the bounded raw working set.
            .clamp(1, 4)
    } else {
        1
    };
    collect_using(source, geometry, check, progress, workers, 4 * 1024 * 1024)
}

fn collect_using(
    source: &mut (impl layout::Source + journal::Source),
    geometry: Geometry,
    check: impl Fn() -> layout::Result<()>,
    mut progress: impl FnMut(Progress),
    workers: usize,
    batch_bytes: usize,
) -> layout::Result<Snapshot> {
    check()?;
    let start = source.checkpoint()?.validate()?;
    let layout = MftLayout::load(source, geometry, &check)?;
    let mut builder = CatalogBuilder::default();
    let mut repair = BTreeSet::new();
    let total = geometry.mft_bytes / u64::from(geometry.record_size);
    if workers > 1 {
        parallel::scan(
            &layout,
            source,
            geometry,
            &mut builder,
            &mut repair,
            &check,
            |processed| progress(Progress::Mft { processed, total }),
            workers,
            batch_bytes,
        )?;
    } else {
        layout.scan(
            source,
            |number, bytes| {
                match parse_record(number, geometry.sector_size as usize, bytes) {
                    Ok(Some(record)) => builder.insert(record)?,
                    Ok(None) => {}
                    Err(_) => {
                        // A live volume can change a sector between reads. Re-read
                        // damaged records through the driver after the bulk pass;
                        // do not hide the problem or restart the entire scan per file.
                        repair.insert(number);
                        if repair.len() > 4096 {
                            return Err(Error::Limit);
                        }
                    }
                }
                Ok(())
            },
            |processed| {
                check()?;
                progress(Progress::Mft { processed, total });
                Ok(())
            },
        )?;
    }
    let stop = source.checkpoint()?.validate()?;
    let changes = journal::collect(source, start, stop, &check)?;
    let replay_total = (changes.references.len() + repair.len()) as u64;
    let mut processed = 0;
    for reference in changes.references {
        check()?;
        match layout::metadata_family(source, geometry, reference, &check) {
            Ok(family) => builder.replace_family(family)?,
            Err(Error::Changed) => builder.remove(reference),
            Err(error) => return Err(error),
        }
        processed += 1;
        progress(Progress::Replay {
            processed,
            total: replay_total,
        });
    }
    for number in repair {
        check()?;
        match layout::metadata_family(source, geometry, FileReference(number), &check) {
            Ok(family) => builder.replace_family(family)?,
            Err(Error::Changed) => builder.discard_number(number),
            Err(error) => return Err(error),
        }
        processed += 1;
        progress(Progress::Replay {
            processed,
            total: replay_total,
        });
    }
    check()?;
    validate_replay_end(source, stop)?;
    let catalog = builder.finish()?;
    check()?;
    Ok(Snapshot {
        catalog,
        checkpoint: changes.checkpoint,
        records_read: total,
        geometry,
    })
}

/// Consume the previous snapshot so a failed/cancelled replay cannot expose a
/// half-updated catalog to its caller. The platform must additionally bind the
/// snapshot to the same kernel device; equal NTFS serials alone are insufficient.
/// Only changed metadata families are fetched; this does not scan the MFT.
pub fn refresh(
    mut previous: Snapshot,
    source: &mut (impl layout::Source + journal::Source),
    geometry: Geometry,
    check: impl Fn() -> layout::Result<()>,
    mut progress: impl FnMut(Progress),
) -> layout::Result<Snapshot> {
    check()?;
    validate_refresh_geometry(previous.geometry, geometry)?;
    let stop = source.checkpoint()?.validate()?;
    let changes = journal::collect(source, previous.checkpoint, stop, &check)?;
    let mut affected = BTreeSet::new();
    for reference in changes.references {
        // Capture old relationships BEFORE changing any family. In particular
        // a deleted/reused extension must refresh its old base as well as the
        // current file that may now occupy its MFT number.
        if let Some(owner) = previous.catalog.owner(reference) {
            affected.insert(owner.0);
        }
        affected.insert(reference.0);
    }
    let total = affected.len() as u64;
    for (index, reference) in affected.into_iter().enumerate() {
        check()?;
        let reference = FileReference(reference);
        match layout::metadata_family(source, geometry, reference, &check) {
            Ok(family) => previous.catalog.replace_family(family)?,
            Err(Error::Changed) => previous.catalog.remove(reference),
            Err(error) => return Err(error),
        }
        progress(Progress::Replay {
            processed: index as u64 + 1,
            total,
        });
    }
    check()?;
    validate_replay_end(source, stop)?;
    previous.checkpoint = changes.checkpoint;
    previous.geometry = geometry;
    previous.records_read = 0;
    Ok(previous)
}

fn validate_refresh_geometry(old: Geometry, geometry: Geometry) -> layout::Result<()> {
    geometry.validate()?;
    if old.serial != geometry.serial
        || old.sector_size != geometry.sector_size
        || old.cluster_size != geometry.cluster_size
        || old.record_size != geometry.record_size
        || old.volume_bytes != geometry.volume_bytes
    {
        return Err(Error::Changed);
    }
    Ok(())
}

fn validate_replay_end(
    source: &mut impl journal::Source,
    stop: journal::Checkpoint,
) -> layout::Result<()> {
    let current = source.checkpoint()?.validate()?;
    if current.id != stop.id
        || current.first.max(current.lowest) > stop.next
        || current.next < stop.next
    {
        return Err(Error::JournalGap);
    }
    Ok(())
}
