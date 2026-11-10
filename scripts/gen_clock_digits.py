#!/usr/bin/env python3
"""Render the screensaver clock's digits to ui/clock/0.png ... 9.png.

Slint pre-renders every character at every font size used, so a 200 px font
for the clock would cost megabytes of flash. Single-colour PNGs are stored
as 1-byte alpha maps instead (about 20 KB each), and Slint can scale them,
which the flip animation needs.

Run once after changing the font or size:
    uv run --with pillow scripts/gen_clock_digits.py
"""
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parent.parent
FONT = ROOT / "ui/fonts/Inter-SemiBold.ttf"
OUT = ROOT / "ui/clock"
SIZE = 210  # px; digits come out about 150 px tall
MARGIN = 4

font = ImageFont.truetype(str(FONT), SIZE)
boxes = {d: font.getbbox(str(d)) for d in range(10)}
top = min(b[1] for b in boxes.values())
bottom = max(b[3] for b in boxes.values())
width = max(b[2] - b[0] for b in boxes.values()) + 2 * MARGIN
height = bottom - top + 2 * MARGIN

OUT.mkdir(exist_ok=True)
for d, (x0, _, x1, _) in boxes.items():
    img = Image.new("RGBA", (width, height), (255, 255, 255, 0))
    draw = ImageDraw.Draw(img)
    # Same baseline for every digit, centred horizontally.
    x = (width - (x1 - x0)) // 2 - x0
    draw.text((x, MARGIN - top), str(d), font=font, fill=(255, 255, 255, 255))
    img.save(OUT / f"{d}.png", optimize=True)
print(f"{width}x{height} px digits written to {OUT}")
