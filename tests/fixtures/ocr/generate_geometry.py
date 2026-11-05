"""Independent geometry fixtures for EXIF and PDF crop/rotation checks.

Pillow/pypdf generate these without Prism, Vision or Quartz. Existing OCR
fixtures are intentionally left untouched (including randomized encrypted PDFs).
"""
from pathlib import Path
from PIL import Image, ImageDraw
from pypdf import PdfWriter
from pypdf.generic import DecodedStreamObject, NameObject, NumberObject, RectangleObject

root = Path(__file__).resolve().parent
picture = Image.new("RGB", (128, 64), "red")
ImageDraw.Draw(picture).rectangle((64, 0, 127, 63), fill="blue")
exif = picture.getexif()
exif[274] = 6  # clockwise 90 degrees: red becomes the displayed top half.
picture.save(root / "exif-clockwise.jpg", quality=100, subsampling=0, exif=exif)

writer = PdfWriter()
page = writer.add_blank_page(width=400, height=200)
page[NameObject("/CropBox")] = RectangleObject((100, 50, 300, 150))
page[NameObject("/Rotate")] = NumberObject(90)
stream = DecodedStreamObject()
stream.set_data(b"0 1 0 rg 0 0 400 200 re f\n1 0 0 rg 100 50 100 100 re f\n0 0 1 rg 200 50 100 100 re f\n")
page[NameObject("/Contents")] = writer._add_object(stream)
with (root / "crop-clockwise.pdf").open("wb") as file:
    writer.write(file)
