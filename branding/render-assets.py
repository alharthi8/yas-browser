"""Render Yas's original SVG into Windows icons. Requires PyMuPDF and Pillow."""
import io
import pathlib
import struct
import pymupdf as fitz
from PIL import Image

ROOT = pathlib.Path(__file__).resolve().parent
svg = (ROOT / 'yas-symbol.svg').read_bytes()
document = fitz.open(stream=svg, filetype='svg')
pixmap = document[0].get_pixmap(matrix=fitz.Matrix(8, 8), alpha=True)
master = Image.open(io.BytesIO(pixmap.tobytes('png'))).convert('RGBA')
master.save(ROOT / 'yas-logo.png')
entries = []
for size in (16, 20, 24, 32, 40, 48, 64, 128, 256):
    bitmap = master.resize((size, size), Image.Resampling.LANCZOS)
    stream = io.BytesIO()
    bitmap.save(stream, format='PNG')
    entries.append((size, stream.getvalue()))
    if size == 64:
        (ROOT / 'icon.rgba').write_bytes(bitmap.tobytes())
header = struct.pack('<HHH', 0, 1, len(entries))
offset = 6 + 16 * len(entries)
table = bytearray()
for size, data in entries:
    encoded = size if size < 256 else 0
    table.extend(struct.pack('<BBBBHHII', encoded, encoded, 0, 0, 1, 32, len(data), offset))
    offset += len(data)
(ROOT / 'yas.ico').write_bytes(header + bytes(table) + b''.join(data for _, data in entries))
print('Rendered Yas falcon logo, 9 Windows icon sizes, and 64px RGBA.')
