# Independent NTFS fixture

`testfs1-mft.gz` contains only the logical MFT extracted from Colin Finck's
public `ntfs` 0.4.0 `testdata/testfs1` image, created with `mkntfs` and `ntfs-3g`.
It contains 581 records (1,024 bytes each, 512-byte sectors). The compressed
fixture is 13,804 bytes; no local or user disk was used.

`testfs1-volume.gz` is the complete same 2,097,152-byte public image, compressed
to 146,079 bytes. The volume fixture verifies bootstrapping the MFT runlist and
bulk physical reads before decoding the records. The test source cannot fetch
extension records, so it cannot accidentally stand in for a live Windows driver.

- Source revision: `3110cafc0440a84f2593b5d3691e0b23b68ccb8f`.
- [Original image](https://github.com/ColinFinck/ntfs/blob/3110cafc0440a84f2593b5d3691e0b23b68ccb8f/testdata/testfs1).
- [Upstream creation script](https://github.com/ColinFinck/ntfs/blob/3110cafc0440a84f2593b5d3691e0b23b68ccb8f/testdata/create-testfs1.sh).
- Original image SHA-256: `e3612c182b8010e3599b5eb93bff427c7d824e85bdc2ddbe46e378e3ba814eb9`.
- Uncompressed MFT SHA-256: `2809b89d98e7db8b1613a7a9ad26aa5400840054d005d8293fde00c229d0f5b4`.
- Copyright 2021 Colin Finck; used under the included `LICENSE-MIT-ntfs`.

Regenerate with `python extract_fixture.py /path/to/ntfs-0.4.0/testdata/testfs1`.
The generator checks the input digest and supports only this image's layout.
It is not a production volume reader. Ground truth comes from the upstream
creation script: a zero-byte file, resident five-byte file, nonresident
1,000-byte file, sparse 500,005-byte file, and 512 numbered subdirectories.

Passing this fixture proves record/path decoding on an independent filesystem
image. It does not prove live MFT enumeration, journal replay, service security,
installation, or whole-drive performance.
