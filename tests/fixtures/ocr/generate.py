"""Independent pixel-only OCR fixtures; Pillow + pypdf + cryptography tooling."""
from pathlib import Path
from PIL import Image, ImageDraw, ImageFont
from pypdf import PdfReader, PdfWriter
from pypdf.generic import DictionaryObject, NameObject, NumberObject, DecodedStreamObject

ROOT = Path(__file__).resolve().parent
FONT = ImageFont.truetype("C:/Windows/Fonts/malgun.ttf", 84)
ENGLISH = "PRISM LOCAL SEARCH 74829"
KOREAN = "프리즘 문서 검색 74829"
for alpha in [False, True]:
    image = Image.new("RGBA" if alpha else "RGB", (1600, 430), (0, 0, 0, 0) if alpha else "white")
    draw = ImageDraw.Draw(image)
    draw.text((70, 60), ENGLISH, font=FONT, fill=(0, 0, 0, 255) if alpha else "black")
    draw.text((70, 230), KOREAN, font=FONT, fill=(0, 0, 0, 255) if alpha else "black")
    image.save(ROOT / ("transparent.png" if alpha else "bilingual.png"))

image = Image.open(ROOT / "bilingual.png").convert("RGB")
for name, mixed, locked in [("scan.pdf", False, False), ("mixed.pdf", True, False), ("scan-locked.pdf", False, True)]:
    writer = PdfWriter()
    if mixed:
        page = writer.add_blank_page(width=800, height=215)
        font = writer._add_object(DictionaryObject({NameObject("/Type"): NameObject("/Font"), NameObject("/Subtype"): NameObject("/Type1"), NameObject("/BaseFont"): NameObject("/Helvetica")}))
        page[NameObject("/Resources")] = DictionaryObject({NameObject("/Font"): DictionaryObject({NameObject("/F1"): font})})
        stream = DecodedStreamObject()
        stream.set_data(b"BT /F1 16 Tf 40 100 Td (typed original page one) Tj ET")
        page[NameObject("/Contents")] = writer._add_object(stream)
    page = writer.add_blank_page(width=800, height=215)
    picture = DecodedStreamObject()
    picture.set_data(image.tobytes())
    picture.update({NameObject("/Type"): NameObject("/XObject"), NameObject("/Subtype"): NameObject("/Image"), NameObject("/Width"): NumberObject(image.width), NameObject("/Height"): NumberObject(image.height), NameObject("/ColorSpace"): NameObject("/DeviceRGB"), NameObject("/BitsPerComponent"): NumberObject(8)})
    page[NameObject("/Resources")] = DictionaryObject({NameObject("/XObject"): DictionaryObject({NameObject("/Scan"): writer._add_object(picture.flate_encode())})})
    content = DecodedStreamObject()
    content.set_data(b"q 800 0 0 215 0 0 cm /Scan Do Q")
    page[NameObject("/Contents")] = writer._add_object(content)
    if mixed:
        page = writer.add_blank_page(width=800, height=215)
        page[NameObject("/Resources")] = DictionaryObject({NameObject("/Font"): DictionaryObject({NameObject("/F1"): font})})
        typed = DecodedStreamObject()
        typed.set_data(b"BT /F1 16 Tf 40 100 Td (typed original page three) Tj ET")
        page[NameObject("/Contents")] = writer._add_object(typed)
        writer.add_blank_page(width=800, height=215)
    if locked:
        writer.encrypt("ocr-fixture-password", "ocr-fixture-owner", algorithm="AES-256")
    path = ROOT / name
    with path.open("wb") as output:
        writer.write(output)
    reader = PdfReader(path)
    if locked:
        assert reader.decrypt("ocr-fixture-password")
    scanned = [reader.pages[1], reader.pages[3]] if mixed else reader.pages
    assert all(not page.extract_text().strip() for page in scanned)
for name in ["bilingual.png", "transparent.png"]:
    assert ENGLISH.encode() not in (ROOT / name).read_bytes()
    assert KOREAN.encode() not in (ROOT / name).read_bytes()
