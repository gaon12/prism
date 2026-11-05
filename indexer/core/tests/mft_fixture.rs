//! Independent, mkntfs/ntfs-3g-created public filesystem fixture, not bytes
//! synthesized by the parser under test. See fixtures/README.md for provenance.
use flate2::read::GzDecoder;
use prism_ntfs_core::{parse_record, CatalogBuilder};
use prism_ntfs_protocol::DriveLetter;
use std::io::Read;

#[test]
fn parses_the_public_mkntfs_image_without_reading_user_file_streams() {
    let mut bytes = vec![];
    GzDecoder::new(&include_bytes!("fixtures/testfs1-mft.gz")[..])
        .take(2 * 1024 * 1024)
        .read_to_end(&mut bytes)
        .unwrap();
    assert!(bytes.len().is_multiple_of(1024));
    let mut builder = CatalogBuilder::default();
    for (number, record) in bytes.as_chunks::<1024>().0.iter().enumerate() {
        if let Some(record) = parse_record(number as u64, 512, record)
            .unwrap_or_else(|error| panic!("record {number}: {error:?}"))
        {
            builder.insert(record).unwrap();
        }
    }
    let mut files = vec![];
    builder
        .finish()
        .unwrap()
        .visit(DriveLetter::new(b'C').unwrap(), |record| {
            files.push(record);
            Ok::<_, ()>(())
        })
        .unwrap();
    let get = |path: &str| files.iter().find(|record| record.path == path).unwrap();
    assert_eq!(get("C:\\empty-file").size, 0);
    assert_eq!(get("C:\\file-with-12345").size, 5);
    assert_eq!(get("C:\\1000-bytes-file").size, 1000);
    assert_eq!(get("C:\\sparse-file").size, 500005);
    assert!(get("C:\\empty-file").modified_ms.is_some());
    for number in 1..=512 {
        assert!(get(&format!("C:\\many_subdirs\\{number}")).is_dir);
    }
    assert_eq!(
        files
            .iter()
            .filter(|record| record.path.starts_with("C:\\many_subdirs\\"))
            .count(),
        512
    );
}
