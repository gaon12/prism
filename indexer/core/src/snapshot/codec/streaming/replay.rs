//! Refresh only changed families without reloading every cached file name.
use super::*;
use crate::{journal, layout, snapshot::Progress};
use std::collections::BTreeSet;

impl<R: Read + Seek> Reader<R> {
    /// Consume the reader so cancellation, inconsistent family reads or a
    /// replaced journal cannot expose a half-refreshed snapshot. The caller
    /// must still bind the volume to the original protected cache's device.
    /// Unchanged file metadata remains in the immutable serialized input.
    pub fn refresh(
        mut self,
        source: &mut (impl layout::Source + journal::Source),
        geometry: Geometry,
        check: impl Fn() -> layout::Result<()>,
        mut progress: impl FnMut(Progress),
    ) -> layout::Result<Self> {
        check()?;
        super::super::super::validate_refresh_geometry(self.geometry(), geometry)?;
        let stop = source.checkpoint()?.validate()?;
        let changes = journal::collect(source, self.checkpoint(), stop, &check)?;
        let mut affected = BTreeSet::new();
        for reference in changes.references {
            // Capture ALL old owners before replacing any family. A deleted
            // extension's old base still needs to lose its old name/size.
            if let Some(owner) = self.owner(reference) {
                affected.insert(owner.0);
            }
            affected.insert(reference.0);
        }
        let total = affected.len() as u64;
        for (index, reference) in affected.into_iter().enumerate() {
            check()?;
            let reference = FileReference(reference);
            match layout::metadata_family(source, geometry, reference, &check) {
                Ok(family) => self.replace_family(family)?,
                Err(layout::Error::Changed) => {
                    if self.reference(reference.number()) == Some(reference) {
                        self.remove_number(reference.number());
                    }
                }
                Err(error) => return Err(error),
            }
            if self.overrides.len() > MAX_RECORDS {
                return Err(layout::Error::Limit);
            }
            progress(Progress::Replay {
                processed: index as u64 + 1,
                total,
            });
        }
        check()?;
        super::super::super::validate_replay_end(source, stop)?;
        // The overlay and directory graph now describe the new checkpoint.
        // Reject an ownership conflict instead of persisting ambiguous data.
        for (index, (&member, &base)) in self.directories.owners.iter().enumerate() {
            if index % 256 == 0 {
                check()?;
            }
            if self.reference(member).is_some() || self.reference(base.number()) != Some(base) {
                return Err(layout::Error::Inconsistent);
            }
        }
        check()?;
        self.header.geometry = geometry;
        self.header.checkpoint = changes.checkpoint;
        Ok(self)
    }

    fn remove_number(&mut self, number: u64) {
        if let Some(reference) = self.reference(number) {
            if self.is_directory(number) {
                self.directory_records -= 1;
            }
            self.records -= 1;
            if let Some(members) = self.directories.families.remove(&reference) {
                for member in members {
                    if self.directories.owners.get(&member) == Some(&reference) {
                        self.directories.owners.remove(&member);
                    }
                }
            }
            self.overrides.insert(number, None);
        }
    }

    fn replace_family(&mut self, family: Vec<MetadataRecord>) -> layout::Result<()> {
        if self
            .records
            .saturating_add(self.directories.owners.len())
            .saturating_add(family.len())
            > MAX_RECORDS
        {
            return Err(layout::Error::Limit);
        }
        // Use the full catalog's family validation, alias filtering, name
        // budget and metadata merge rather than a second implementation.
        let mut merged = Catalog {
            records: Records::default(),
            directories: HashMap::new(),
            owners: HashMap::new(),
            families: HashMap::new(),
        };
        merged.replace_family(family)?;
        let number = *merged.records.iter().next().unwrap().0;
        let record = merged.records.remove(&number).unwrap();
        self.remove_number(number);
        for &member in merged.owners.keys() {
            // A former base can be reused as another file's extension.
            self.remove_number(member);
        }
        self.directories.owners.extend(merged.owners);
        self.directories.families.extend(merged.families);
        self.records += 1;
        if record.is_dir {
            self.directory_records += 1;
        }
        self.overrides.insert(number, Some(record));
        self.directories.directories.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::tests::{encoded, sample, DEVICE};
    use super::*;
    use std::io::Cursor;

    #[test]
    fn mixed_family_overlay_matches_full_metadata_order_paths_and_ownership() {
        let mut full = sample();
        let template = full.catalog.records[&18].clone();
        let mut unchanged = template.clone();
        unchanged.reference = FileReference((1 << 48) | 19);
        unchanged.names[0].name = "unchanged.md".into();
        full.catalog.records.insert(19, unchanged);
        let mut stream = Reader::open(
            Cursor::new(encoded(&full)),
            full.geometry,
            DEVICE,
            || Ok(()),
        )
        .unwrap();
        let mut seed = 0x614b8d39u64;
        for step in 0..400 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let number = if step % 17 == 0 { 18 } else { 60 + seed % 31 };
            if seed & 3 == 0 {
                if let Some(reference) = stream.reference(number) {
                    full.catalog.remove(reference);
                    stream.remove_number(number);
                }
            } else {
                let mut base = template.clone();
                base.reference = FileReference((((seed >> 32) & 0xffff) << 48) | number);
                base.is_dir = seed & 4 == 0;
                base.size = Some(if base.is_dir { 0 } else { seed });
                base.modified_ms = match step % 3 {
                    0 => None,
                    1 => Some(i64::MIN),
                    _ => Some(seed as i64),
                };
                base.names.truncate(1);
                base.names[0].name = format!("자료-日本語-😀-{step}");
                base.names[0].parent = if seed & 8 == 0 {
                    stream
                        .reference(60 + (seed >> 8) % 31)
                        .unwrap_or(FileReference(0))
                } else {
                    full.catalog.records[&5].reference
                };
                let mut family = vec![base.clone()];
                if seed.is_multiple_of(5) {
                    let mut extension = base.clone();
                    extension.reference = FileReference((2 << 48) | (1000 + number));
                    extension.base = Some(base.reference);
                    extension.names[0].parent = full.catalog.records[&5].reference;
                    extension.names[0].name = format!("linked-{step}");
                    family.push(extension);
                }
                full.catalog.replace_family(family.clone()).unwrap();
                stream.replace_family(family).unwrap();
            }
            assert_eq!(stream.record_count(), full.catalog.records.len());
            assert_eq!(
                stream.directory_count(),
                full.catalog
                    .records
                    .values()
                    .filter(|record| record.is_dir)
                    .count()
            );
            assert_eq!(stream.directories.owners, full.catalog.owners);
            assert_eq!(stream.directories.families, full.catalog.families);
            let mut expected = Vec::new();
            for (_, record) in full.catalog.records.iter() {
                let mut bytes = Vec::new();
                encode_record(record, &mut bytes).unwrap();
                expected.push(bytes);
            }
            let mut actual = Vec::new();
            stream
                .visit_metadata(
                    || Ok(()),
                    |record| {
                        let mut bytes = Vec::new();
                        encode_record(&record, &mut bytes)?;
                        actual.push(bytes);
                        Ok(())
                    },
                )
                .unwrap();
            assert_eq!(actual, expected, "metadata at step {step}");
            let drive = DriveLetter::new(if step % 2 == 0 { b'C' } else { b'D' }).unwrap();
            let mut expected = Vec::new();
            full.catalog
                .visit(drive, |row| {
                    expected.push(row);
                    Ok::<_, ()>(())
                })
                .unwrap();
            let mut actual = Vec::new();
            stream
                .visit(
                    drive,
                    || Ok(()),
                    |row| {
                        actual.push(row);
                        Ok(())
                    },
                )
                .unwrap();
            assert_eq!(actual, expected, "paths at step {step}");
            // Neither the directory graph nor the overlay retains this
            // unchanged file name; it is decoded from the immutable input.
            assert!(!stream.directories.records.contains_key(&19));
            assert!(!stream.overrides.contains_key(&19));
            assert!(!stream.is_directory(19));
        }
        let mut saved = Cursor::new(Vec::new());
        stream.write(DEVICE, &mut saved, || Ok(())).unwrap();
        assert_eq!(
            saved.into_inner(),
            encoded(&full),
            "overlay persistence retains every metadata field and owner in the unchanged format"
        );
    }
}
