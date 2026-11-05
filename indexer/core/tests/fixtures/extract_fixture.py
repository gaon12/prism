"""Extract only the public ntfs-0.4.0 test image's logical MFT.

This fixture generator is not a production volume reader. It deliberately
accepts one pinned image, and rejects attribute lists or unusual layouts.
Usage: python extract_fixture.py /path/to/ntfs-0.4.0/testdata/testfs1
"""
import gzip
import hashlib
import pathlib
import struct
import sys

image = pathlib.Path(sys.argv[1]).read_bytes()
assert hashlib.sha256(image).hexdigest() == "e3612c182b8010e3599b5eb93bff427c7d824e85bdc2ddbe46e378e3ba814eb9"
assert image[3:11] == b"NTFS    "
sector = struct.unpack_from("<H", image, 11)[0]
cluster = sector * image[13]
encoded_size = struct.unpack_from("<b", image, 64)[0]
record_size = (1 << -encoded_size) if encoded_size < 0 else encoded_size * cluster
location = struct.unpack_from("<Q", image, 48)[0] * cluster
record = bytearray(image[location:location + record_size])
usa, count = struct.unpack_from("<HH", record, 4)
for n in range(1, count):
    end = n * sector - 2
    assert record[end:end+2] == record[usa:usa+2]
    record[end:end+2] = record[usa+n*2:usa+n*2+2]
offset = struct.unpack_from("<H", record, 20)[0]
logical = None
while struct.unpack_from("<I", record, offset)[0] != 0xffffffff:
    kind, length = struct.unpack_from("<II", record, offset)
    assert length >= 24
    assert kind != 0x20, "pinned fixture must not need attribute-list resolution"
    if kind == 0x80 and record[offset + 9] == 0:
        assert record[offset+8] == 1
        size = struct.unpack_from("<Q", record, offset+48)[0]
        run = offset + struct.unpack_from("<H", record, offset+32)[0]
        lcn = 0
        chunks = []
        while record[run]:
            header = record[run]
            run += 1
            length_size, offset_size = header & 15, header >> 4
            assert length_size and offset_size
            clusters = int.from_bytes(record[run:run+length_size], "little")
            run += length_size
            lcn += int.from_bytes(record[run:run+offset_size], "little", signed=True)
            run += offset_size
            chunks.append(image[lcn*cluster:(lcn+clusters)*cluster])
        logical = b"".join(chunks)[:size]
        break
    offset += length
assert logical and len(logical) % record_size == 0
destination = pathlib.Path(__file__).with_name("testfs1-mft.gz")
destination.write_bytes(gzip.compress(logical, mtime=0))
pathlib.Path(__file__).with_name("testfs1-volume.gz").write_bytes(gzip.compress(image, mtime=0))
print({"records": len(logical)//record_size, "recordSize": record_size, "sectorSize": sector, "mftBytes": len(logical), "gzipBytes": destination.stat().st_size, "sha256": hashlib.sha256(logical).hexdigest()})
