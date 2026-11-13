//! Read-only path fields, not another owned directory metadata catalog.
use crate::{DirectoryLookup, DirectoryView, FileReference, MetadataRecord};
use std::{collections::BTreeMap, io};
const PAGE: usize = 1024;
const BLOCK: usize = 65536;
const HAS_NAME: u16 = 1;
const ALIAS: u16 = 2;
struct Node {
    reference: FileReference,
    parent: FileReference,
    name: u32,
    length: u16,
    flags: u16,
}
#[derive(Default)]
struct Names {
    blocks: Vec<Vec<u8>>,
}
impl Names {
    fn push(&mut self, name: &str) -> io::Result<(u32, u16)> {
        let length = u16::try_from(name.len()).map_err(|_| io::ErrorKind::InvalidData)?;
        if self
            .blocks
            .last()
            .is_none_or(|block| block.len() == BLOCK || block.len() + name.len() > BLOCK)
        {
            if self.blocks.len() > u16::MAX as usize {
                return Err(io::ErrorKind::InvalidData.into());
            }
            self.blocks.push(Vec::with_capacity(BLOCK));
        }
        let index = self.blocks.len() - 1;
        let block = &mut self.blocks[index];
        let offset = ((index as u32) << 16) | block.len() as u32;
        block.extend_from_slice(name.as_bytes());
        Ok((offset, length))
    }
    fn get(&self, offset: u32, length: u16) -> &str {
        let block = &self.blocks[(offset >> 16) as usize];
        let start = (offset & 0xffff) as usize;
        // Constructed only by copying validated str bytes into one block.
        std::str::from_utf8(&block[start..start + usize::from(length)])
            .expect("Validated directory UTF-8")
    }
}
#[derive(Default)]
pub(super) struct Index {
    pages: Vec<Vec<Node>>,
    len: usize,
    names: Names,
}
impl Index {
    pub(super) fn len(&self) -> usize {
        self.len
    }
    fn node(&self, index: usize) -> &Node {
        &self.pages[index / PAGE][index % PAGE]
    }
    pub(super) fn push(&mut self, record: &MetadataRecord) -> io::Result<()> {
        if !record.is_dir
            || self.len > 0
                && self.node(self.len - 1).reference.number() >= record.reference.number()
        {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let mut node = Node {
            reference: record.reference,
            parent: FileReference(0),
            name: 0,
            length: 0,
            flags: if record.alias() { ALIAS } else { 0 },
        };
        if let Some(first) = record.names.first() {
            let (offset, length) = self.names.push(&first.name)?;
            node.parent = first.parent;
            node.name = offset;
            node.length = length;
            node.flags |= HAS_NAME;
        }
        if self.pages.last().is_none_or(|page| page.len() == PAGE) {
            self.pages.push(Vec::with_capacity(PAGE));
        }
        self.pages.last_mut().unwrap().push(node);
        self.len += 1;
        Ok(())
    }
}
impl DirectoryLookup for Index {
    fn directory(&self, number: u64) -> Option<DirectoryView<'_>> {
        let (mut low, mut high) = (0, self.len);
        while low < high {
            let middle = low + (high - low) / 2;
            let node = self.node(middle);
            match node.reference.number().cmp(&number) {
                std::cmp::Ordering::Less => low = middle + 1,
                std::cmp::Ordering::Greater => high = middle,
                std::cmp::Ordering::Equal => {
                    return Some(DirectoryView {
                        reference: node.reference,
                        alias: node.flags & ALIAS != 0,
                        first_name: (node.flags & HAS_NAME != 0)
                            .then(|| (node.parent, self.names.get(node.name, node.length))),
                    })
                }
            }
        }
        None
    }
}
pub(super) struct View<'a> {
    pub(super) base: &'a Index,
    pub(super) changes: &'a BTreeMap<u64, Option<MetadataRecord>>,
}
impl DirectoryLookup for View<'_> {
    fn directory(&self, number: u64) -> Option<DirectoryView<'_>> {
        match self.changes.get(&number) {
            Some(record) => record.as_ref().and_then(DirectoryView::from_record),
            None => self.base.directory(number),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::tests::{encoded, sample, DEVICE};
    use super::*;
    use crate::{
        snapshot::{codec::streaming::Reader, Snapshot},
        CatalogBuilder, FileName, Records,
    };
    use std::io::Cursor;
    fn entry(
        number: u64,
        sequence: u64,
        directory: bool,
        parent: FileReference,
        name: Option<&str>,
    ) -> MetadataRecord {
        let mut record = sample().catalog.records[&18].clone();
        record.reference = FileReference((sequence << 48) | number);
        record.base = None;
        record.is_dir = directory;
        record.size = Some(if directory { 0 } else { 7 });
        record.modified_ms = Some(number as i64);
        record.attributes = 0;
        record.reparse_tag = None;
        record.names = name.map_or_else(Vec::new, |name| {
            vec![FileName {
                parent,
                name: name.into(),
                namespace: 1,
            }]
        });
        record
    }
    #[test]
    fn paged_nodes_and_chunked_utf8_names_match_full_directory_views() {
        let mut full = Records::default();
        let mut compact = Index::default();
        let root = sample().catalog.records[&5].clone();
        compact.push(&root).unwrap();
        full.insert(5, root);
        for index in 0..4097 {
            let number = 16 + index;
            let name = format!("資料-😀-{index:05}-{}", "x".repeat(60));
            let mut record = entry(
                number,
                1 + index % 251,
                true,
                FileReference((1 << 48) | 5),
                Some(&name),
            );
            if index % 3 == 0 {
                record.attributes = 0x400;
                record.reparse_tag = if index % 2 == 0 {
                    Some(0x9000001a)
                } else {
                    None
                };
            }
            compact.push(&record).unwrap();
            full.insert(number, record);
        }
        full.insert(
            50000,
            entry(50000, 1, false, FileReference((1 << 48) | 5), Some("file")),
        );
        for number in [0, 5, 15, 4113, 50000, 50001].into_iter().chain(16..4113) {
            assert_eq!(
                compact.directory(number),
                full.directory(number),
                "directory {number}"
            );
        }
    }
    #[test]
    fn compact_paths_keep_alias_sequence_cycle_orphan_first_name_and_root_rules() {
        for root_alias in [false, true] {
            let original = sample();
            let root_ref = original.catalog.records[&5].reference;
            let mut root = original.catalog.records[&5].clone();
            if root_alias {
                root.attributes = 0x400;
                root.reparse_tag = None;
            }
            let mut alias = entry(17, 3, true, root_ref, Some("alias"));
            alias.attributes = 0x400;
            let mut multi = entry(22, 1, true, root_ref, Some("primary"));
            multi.names.push(FileName {
                parent: root_ref,
                name: "secondary".into(),
                namespace: 1,
            });
            let mut cloud = entry(24, 1, true, root_ref, Some("cloud"));
            cloud.attributes = 0x400;
            cloud.reparse_tag = Some(0x9000001a);
            let records = vec![
                root,
                entry(16, 2, true, root_ref, Some("folder")),
                alias,
                entry(18, 5, true, root_ref, Some("reused")),
                entry(19, 1, true, FileReference((1 << 48) | 20), Some("left")),
                entry(20, 1, true, FileReference((1 << 48) | 19), Some("right")),
                entry(21, 1, true, FileReference((1 << 48) | 63), Some("orphan")),
                multi,
                entry(23, 1, true, root_ref, None),
                cloud,
                entry(25, 1, false, root_ref, Some("not-dir")),
                entry(32, 1, false, FileReference((2 << 48) | 16), Some("doc")),
                entry(
                    33,
                    1,
                    false,
                    FileReference((3 << 48) | 17),
                    Some("hidden-alias"),
                ),
                entry(34, 1, false, FileReference((4 << 48) | 18), Some("stale")),
                entry(35, 1, false, FileReference((1 << 48) | 19), Some("cycle")),
                entry(
                    36,
                    1,
                    false,
                    FileReference((1 << 48) | 21),
                    Some("orphan-child"),
                ),
                entry(37, 1, false, FileReference((1 << 48) | 22), Some("child")),
                entry(38, 1, false, FileReference((1 << 48) | 23), Some("unnamed")),
                entry(39, 1, false, FileReference((1 << 48) | 24), Some("online")),
                entry(
                    40,
                    1,
                    false,
                    FileReference((1 << 48) | 25),
                    Some("not-dir-child"),
                ),
            ];
            let mut builder = CatalogBuilder::default();
            for record in records {
                builder.insert(record).unwrap();
            }
            let mut full = Snapshot {
                catalog: builder.finish().unwrap(),
                geometry: original.geometry,
                checkpoint: original.checkpoint,
                records_read: 0,
            };
            let count = full
                .catalog
                .records
                .values()
                .filter(|record| record.is_dir)
                .count();
            let mut reader = Reader::open(
                Cursor::new(encoded(&full)),
                full.geometry,
                DEVICE,
                || Ok(()),
            )
            .unwrap();
            assert_eq!(reader.directory_count(), count);
            assert!(reader.directories.records.is_empty());
            for letter in *b"CDC" {
                let drive = prism_ntfs_protocol::DriveLetter::new(letter).unwrap();
                let mut expected = Vec::new();
                full.catalog
                    .visit(drive, |row| {
                        expected.push(row);
                        Ok::<_, ()>(())
                    })
                    .unwrap();
                let mut actual = Vec::new();
                reader
                    .visit(
                        drive,
                        || Ok(()),
                        |row| {
                            actual.push(row);
                            Ok(())
                        },
                    )
                    .unwrap();
                assert_eq!(actual, expected);
                assert_eq!(actual.is_empty(), root_alias);
                if !root_alias {
                    let prefix = drive.root();
                    for suffix in [
                        "folder\\doc",
                        "primary\\child",
                        "secondary",
                        "cloud\\online",
                    ] {
                        assert!(actual
                            .iter()
                            .any(|row| row.path == format!("{prefix}{suffix}")));
                    }
                    for suffix in [
                        "secondary\\child",
                        "alias\\hidden-alias",
                        "reused\\stale",
                        "not-dir\\not-dir-child",
                    ] {
                        assert!(!actual
                            .iter()
                            .any(|row| row.path == format!("{prefix}{suffix}")));
                    }
                }
            }
        }
    }
}
