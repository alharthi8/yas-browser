"""Render the Yas wordmark with the installed Dubai Bold font; no font is bundled."""
import argparse
import io
import math
import os
import pathlib
import pymupdf
from PIL import Image, ImageDraw, ImageFont

ROOT = pathlib.Path(__file__).resolve().parent
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--font', type=pathlib.Path, default=pathlib.Path(os.environ.get('WINDIR', 'C:/Windows')) / 'Fonts/DUBAI-BOLD.TTF')
args = parser.parse_args()
if not args.font.is_file():
    raise SystemExit('Dubai Bold is not installed; pass --font with its local font path.')
width, height, supersampling = 700, 285, 4
scale = width / 315 * supersampling
document = pymupdf.open(stream=(ROOT / 'yas-symbol.svg').read_bytes(), filetype='svg')
pixmap = document[0].get_pixmap(matrix=pymupdf.Matrix(scale, scale), alpha=True)
symbol = Image.open(io.BytesIO(pixmap.tobytes('png'))).convert('RGBA')
canvas = Image.new('RGBA', (width * supersampling, height * supersampling), (0, 0, 0, 0))
canvas.alpha_composite(symbol, (0, 0))
font = ImageFont.truetype(str(args.font), round(76 * scale))
ImageDraw.Draw(canvas).text((155 * scale, 87 * scale), 'Yas', font=font, fill='#192c49', anchor='ls')
canvas.resize((width, height), Image.Resampling.LANCZOS).save(ROOT / 'yas-wordmark.png')
print(f'Rendered transparent {width}x{height} Yas wordmark with {font.getname()[0]} {font.getname()[1]}.')
