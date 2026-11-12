//! Dense, ordered metadata pages with a small overlay for out-of-order replay.
//! Pages never move earlier records when an ordered MFT/cache stream grows.
use crate::MetadataRecord;
use std::{collections::BTreeMap, ops::Index};

const PAGE: usize = 1024;

struct Slot {
    number: u64,
    record: Option<MetadataRecord>,
}

#[derive(Default)]
pub(crate) struct Records {
    pages: Vec<Vec<Slot>>,
    base_len: usize,
    tombstones: usize,
    replay: BTreeMap<u64, MetadataRecord>,
    len: usize,
}

impl Records {
    #[cfg(any(feature = "cache", test))]
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            pages: Vec::with_capacity(capacity.div_ceil(PAGE)),
            ..Self::default()
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn slot(&self, index: usize) -> &Slot {
        &self.pages[index / PAGE][index % PAGE]
    }

    fn slot_mut(&mut self, index: usize) -> &mut Slot {
        &mut self.pages[index / PAGE][index % PAGE]
    }

    fn find(&self, number: u64) -> Option<usize> {
        // Ordered collection inserts/removes the next number: avoid a binary
        // search through every earlier record for this overwhelmingly common case.
        if self.base_len == 0 || number > self.slot(self.base_len - 1).number {
            return None;
        }
        let (mut low, mut high) = (0, self.base_len);
        while low < high {
            let middle = low + (high - low) / 2;
            match self.slot(middle).number.cmp(&number) {
                std::cmp::Ordering::Less => low = middle + 1,
                std::cmp::Ordering::Greater => high = middle,
                std::cmp::Ordering::Equal => return Some(middle),
            }
        }
        None
    }

    pub(crate) fn get(&self, number: &u64) -> Option<&MetadataRecord> {
        match self.find(*number) {
            Some(index) => self.slot(index).record.as_ref(),
            None => self.replay.get(number),
        }
    }

    pub(crate) fn get_mut(&mut self, number: &u64) -> Option<&mut MetadataRecord> {
        match self.find(*number) {
            Some(index) => self.slot_mut(index).record.as_mut(),
            None => self.replay.get_mut(number),
        }
    }

    pub(crate) fn contains_key(&self, number: &u64) -> bool {
        self.get(number).is_some()
    }

    fn append(&mut self, number: u64, record: MetadataRecord) {
        debug_assert!(self.base_len == 0 || self.slot(self.base_len - 1).number < number);
        if self.pages.last().is_none_or(|page| page.len() == PAGE) {
            self.pages.push(Vec::with_capacity(PAGE));
        }
        self.pages.last_mut().unwrap().push(Slot {
            number,
            record: Some(record),
        });
        self.base_len += 1;
    }

    pub(crate) fn insert(&mut self, number: u64, record: MetadataRecord) -> Option<MetadataRecord> {
        let previous = if self.base_len == 0 || number > self.slot(self.base_len - 1).number {
            // A compaction may have removed trailing slots; an overlay key can
            // then belong beyond the new tail. Move it rather than duplicate it.
            let previous = self.replay.remove(&number);
            self.append(number, record);
            previous
        } else if let Some(index) = self.find(number) {
            let previous = self.slot_mut(index).record.replace(record);
            if previous.is_none() {
                self.tombstones -= 1;
            }
            previous
        } else {
            self.replay.insert(number, record)
        };
        if previous.is_none() {
            self.len += 1;
        }
        self.compact_if_needed();
        previous
    }

    pub(crate) fn remove(&mut self, number: &u64) -> Option<MetadataRecord> {
        let previous = match self.find(*number) {
            Some(index) => {
                let value = self.slot_mut(index).record.take();
                self.tombstones += usize::from(value.is_some());
                value
            }
            None => self.replay.remove(number),
        };
        self.len -= usize::from(previous.is_some());
        self.compact_if_needed();
        previous
    }

    fn compact_if_needed(&mut self) {
        if (self.tombstones > PAGE && self.tombstones >= self.base_len / 2)
            || (self.replay.len() > PAGE && self.replay.len() > self.base_len / 4)
        {
            self.compact();
        }
    }

    fn compact(&mut self) {
        // Consume old pages in order. Each page is freed as it is drained, so
        // compaction does not retain another whole metadata table beside it.
        let mut base = std::mem::take(&mut self.pages)
            .into_iter()
            .flatten()
            .filter_map(|slot| slot.record.map(|record| (slot.number, record)))
            .peekable();
        let mut replay = std::mem::take(&mut self.replay).into_iter().peekable();
        self.base_len = 0;
        self.tombstones = 0;
        loop {
            let next = match (base.peek(), replay.peek()) {
                (Some(left), Some(right)) if left.0 < right.0 => base.next(),
                (Some(_), Some(_)) => replay.next(),
                (Some(_), None) => base.next(),
                (None, Some(_)) => replay.next(),
                (None, None) => break,
            };
            let (number, record) = next.unwrap();
            self.append(number, record);
        }
        debug_assert_eq!(self.base_len, self.len);
    }

    /// Always MFT-number ordered, including pending journal replacements.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&u64, &MetadataRecord)> {
        let mut base = self
            .pages
            .iter()
            .flatten()
            .filter_map(|slot| slot.record.as_ref().map(|record| (&slot.number, record)))
            .peekable();
        let mut replay = self.replay.iter().peekable();
        std::iter::from_fn(move || match (base.peek(), replay.peek()) {
            (Some(left), Some(right)) if left.0 < right.0 => base.next(),
            (Some(_), Some(_)) => replay.next(),
            (Some(_), None) => base.next(),
            (None, Some(_)) => replay.next(),
            (None, None) => None,
        })
    }

    #[cfg(test)]
    pub(crate) fn values(&self) -> impl Iterator<Item = &MetadataRecord> {
        self.iter().map(|(_, record)| record)
    }

    pub(crate) fn values_mut(&mut self) -> impl Iterator<Item = &mut MetadataRecord> {
        self.pages
            .iter_mut()
            .flatten()
            .filter_map(|slot| slot.record.as_mut())
            .chain(self.replay.values_mut())
    }
}

impl Index<&u64> for Records {
    type Output = MetadataRecord;

    fn index(&self, number: &u64) -> &Self::Output {
        self.get(number).expect("metadata number exists")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FileName, FileReference};

    fn record(number: u64, marker: u64) -> MetadataRecord {
        MetadataRecord {
            reference: FileReference(number | ((marker & 0xffff) << 48)),
            base: marker.is_multiple_of(3).then_some(FileReference(5)),
            is_dir: marker.is_multiple_of(5),
            names: vec![FileName {
                parent: FileReference(5),
                name: format!("public-한글-{marker}.md"),
                namespace: (marker % 4) as u8,
            }],
            size: (!marker.is_multiple_of(7)).then_some(marker),
            modified_ms: (!marker.is_multiple_of(11)).then_some(i64::MIN + marker as i64),
            attributes: marker as u32,
            reparse_tag: marker.is_multiple_of(13).then_some(marker as u32),
            name_budget: (marker % 17) as u16,
        }
    }

    fn same(left: Option<&MetadataRecord>, right: Option<&MetadataRecord>) {
        match (left, right) {
            (None, None) => {}
            (Some(left), Some(right)) => assert_eq!(
                (
                    left.reference,
                    left.base,
                    left.is_dir,
                    &left.names,
                    left.size,
                    left.modified_ms,
                    left.attributes,
                    left.reparse_tag,
                    left.name_budget
                ),
                (
                    right.reference,
                    right.base,
                    right.is_dir,
                    &right.names,
                    right.size,
                    right.modified_ms,
                    right.attributes,
                    right.reparse_tag,
                    right.name_budget
                ),
            ),
            _ => panic!("metadata membership differs"),
        }
    }

    #[test]
    fn ordered_pages_and_replay_keep_every_record_and_reused_sequence() {
        let mut records = Records::with_capacity(PAGE * 3);
        for number in 0..(PAGE * 3) as u64 {
            assert!(records
                .insert(number * 2, record(number * 2, number))
                .is_none());
        }
        for number in [0, 2046, 2048, 4094, 4096, 6142] {
            same(
                records.remove(&number).as_ref(),
                Some(&record(number, number / 2)),
            );
            assert!(!records.contains_key(&number));
            records.insert(number, record(number, 65535));
            same(records.get(&number), Some(&record(number, 65535)));
        }
        for number in (1..6144).step_by(2) {
            records.insert(number, record(number, number));
        }
        assert_eq!(records.len(), 6144);
        assert!(records.iter().map(|(&number, _)| number).eq(0..6144));
    }

    #[test]
    fn mutations_match_an_independent_ordered_map() {
        let mut records = Records::default();
        let mut expected = BTreeMap::new();
        let mut random = 0x937a_1bcd_3356_71efu64;
        for step in 0..30_000 {
            random ^= random << 13;
            random ^= random >> 7;
            random ^= random << 17;
            let number = random % 8192;
            match random % 7 {
                0..=2 => same(
                    records.insert(number, record(number, step)).as_ref(),
                    expected.insert(number, record(number, step)).as_ref(),
                ),
                3 => same(
                    records.remove(&number).as_ref(),
                    expected.remove(&number).as_ref(),
                ),
                4 => {
                    for value in [records.get_mut(&number), expected.get_mut(&number)]
                        .into_iter()
                        .flatten()
                    {
                        value.size = Some(u64::MAX - step);
                        value.modified_ms = Some(i64::MAX - step as i64);
                    }
                }
                _ => same(records.get(&number), expected.get(&number)),
            }
            assert_eq!(records.len(), expected.len());
            if step % 499 == 0 {
                assert_eq!(records.iter().count(), expected.len());
                for ((actual_key, actual), (key, reference)) in records.iter().zip(&expected) {
                    assert_eq!(actual_key, key);
                    same(Some(actual), Some(reference));
                }
            }
        }
    }

    #[test]
    fn deletion_history_and_out_of_order_growth_do_not_accumulate_dead_pages() {
        let mut records = Records::default();
        for cycle in 0..8 {
            let first = cycle * 5000;
            for number in first..first + 5000 {
                records.insert(number, record(number, number));
            }
            for number in first..first + 5000 {
                assert!(records.remove(&number).is_some());
            }
            assert!(records.is_empty());
            assert!(records.base_len <= PAGE * 2);
            assert!(records.replay.is_empty());
            assert!(records.iter().next().is_none());
        }
    }
}
