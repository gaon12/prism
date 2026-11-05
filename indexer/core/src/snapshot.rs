//! A snapshot is published only after replaying changes made during the bulk
//! MFT read. Work-in-progress builders never escape on cancellation or errors.
use crate::{
    journal,
    layout::{self, Error, Geometry, MftLayout},
    parse_record, Catalog, CatalogBuilder, FileReference,
};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Progress {
    Mft { processed: u64, total: u64 },
    Replay { processed: u64, total: u64 },
}
pub struct Snapshot {
    pub catalog: Catalog,
    pub checkpoint: journal::Checkpoint,
    pub records_read: u64,
}

pub fn collect(
    source: &mut (impl layout::Source + journal::Source),
    geometry: Geometry,
    check: impl Fn() -> layout::Result<()>,
    mut progress: impl FnMut(Progress),
) -> layout::Result<Snapshot> {
    check()?;
    let start = source.checkpoint()?.validate()?;
    let layout = MftLayout::load(source, geometry, &check)?;
    let mut builder = CatalogBuilder::default();
    let mut repair = BTreeSet::new();
    let total = geometry.mft_bytes / u64::from(geometry.record_size);
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
    let current = source.checkpoint()?.validate()?;
    if current.id != stop.id
        || current.first.max(current.lowest) > stop.next
        || current.next < stop.next
    {
        return Err(Error::JournalGap);
    }
    let catalog = builder.finish()?;
    check()?;
    Ok(Snapshot {
        catalog,
        checkpoint: changes.checkpoint,
        records_read: total,
    })
}
