//! Exercise extent resolution and bulk physical reads on an independent NTFS
//! image. This source is deliberately unable to retrieve nonzero MFT records:
//! the public fixture's MFT does not need attribute-list extension records.
use flate2::read::GzDecoder;
use prism_ntfs_core::{
    layout::{Error, Geometry, MftLayout, RecordBuffer, Source},
    parse_record, CatalogBuilder, FileReference,
};
use prism_ntfs_protocol::DriveLetter;
use std::io::Read;

struct Image {
    bytes: Vec<u8>,
    reads: usize,
    mft_offset: usize,
}
impl Source for Image {
    fn read_exact_at(&mut self, offset: u64, output: &mut [u8]) -> Result<(), Error> {
        let start = usize::try_from(offset).map_err(|_| Error::Corrupt)?;
        let end = start.checked_add(output.len()).ok_or(Error::Corrupt)?;
        output.copy_from_slice(self.bytes.get(start..end).ok_or(Error::Corrupt)?);
        self.reads += 1;
        Ok(())
    }
    fn file_record(&mut self, reference: FileReference) -> Result<RecordBuffer, Error> {
        if reference.number() != 0 {
            return Err(Error::Changed);
        }
        Ok(RecordBuffer {
            bytes: self.bytes[self.mft_offset..self.mft_offset + 1024].to_owned(),
            kernel_fixups: false,
        })
    }
}

#[test]
fn resolves_the_public_volumes_mft_before_collecting_its_metadata() {
    let mut bytes = vec![];
    GzDecoder::new(&include_bytes!("fixtures/testfs1-volume.gz")[..])
        .take(3 * 1024 * 1024)
        .read_to_end(&mut bytes)
        .unwrap();
    assert_eq!(bytes.len(), 2097152);
    let mft_offset = u64::from_le_bytes(bytes[48..56].try_into().unwrap()) as usize * 512;
    let serial = u64::from_le_bytes(bytes[72..80].try_into().unwrap());
    let mut source = Image {
        bytes,
        reads: 0,
        mft_offset,
    };
    let geometry = Geometry {
        sector_size: 512,
        cluster_size: 512,
        record_size: 1024,
        volume_bytes: 2097152,
        mft_bytes: 594944,
        serial,
    };
    let layout = MftLayout::load(&mut source, geometry, || Ok(())).unwrap();
    let mut builder = CatalogBuilder::default();
    let mut total = 0;
    layout
        .scan(
            &mut source,
            |number, bytes| {
                total += 1;
                if let Some(record) = parse_record(number, 512, bytes).unwrap() {
                    builder.insert(record).unwrap();
                }
                Ok(())
            },
            |_| Ok(()),
        )
        .unwrap();
    assert_eq!(total, 581);
    assert!(
        source.reads < 20,
        "bulk reads must not become one syscall per record"
    );
    let mut count = 0;
    builder
        .finish()
        .unwrap()
        .visit(DriveLetter::new(b'C').unwrap(), |file| {
            if file.path.starts_with("C:\\many_subdirs\\") {
                count += 1;
            }
            Ok::<_, ()>(())
        })
        .unwrap();
    assert_eq!(count, 512);
}
