"""Regenerate synthetic PDFs with pypdf + cryptography (test tooling only)."""
from pathlib import Path
import sys

from pypdf import PdfReader, PdfWriter
from pypdf.generic import DecodedStreamObject, DictionaryObject, NameObject

ROOT = Path(__file__).resolve().parent
OWNER_PASSWORD = "fixture-owner-password"  # Public fixture data, not a credential.

for name, algorithm, password in [
    ("rc4-40.pdf", "RC4-40", "fixture-user-password"),
    ("rc4-128.pdf", "RC4-128", "fixture-user-password"),
    ("aes128.pdf", "AES-128", "fixture-user-password"),
    ("aes256.pdf", "AES-256", "암호-fixture-密碼"),
]:
    if len(sys.argv) > 1 and name not in sys.argv[1:]:
        continue
    writer = PdfWriter()
    font = writer._add_object(DictionaryObject({
        NameObject("/Type"): NameObject("/Font"),
        NameObject("/Subtype"): NameObject("/Type1"),
        NameObject("/BaseFont"): NameObject("/Helvetica"),
    }))
    for text in ["unlockcanaryfirst evidence page one", "", "unlockcanarythird evidence page three"]:
        page = writer.add_blank_page(width=612, height=792)
        if text:
            page[NameObject("/Resources")] = DictionaryObject({
                NameObject("/Font"): DictionaryObject({NameObject("/F1"): font}),
            })
            stream = DecodedStreamObject()
            stream.set_data(f"BT /F1 12 Tf 72 720 Td ({text}) Tj ET".encode("ascii"))
            page[NameObject("/Contents")] = writer._add_object(stream)
    writer.encrypt(password, OWNER_PASSWORD, algorithm=algorithm)
    path = ROOT / name
    with path.open("wb") as output:
        writer.write(output)
    for supplied in [password, OWNER_PASSWORD]:
        reader = PdfReader(path)
        assert reader.is_encrypted and reader.decrypt(supplied)
        assert len(reader.pages) == 3
        assert "unlockcanarythird" in reader.pages[2].extract_text()
