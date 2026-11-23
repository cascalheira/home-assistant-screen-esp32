#!/usr/bin/env python3
"""Render the screensaver clock's digits for each board.

Slint makes a distance field per glyph at the largest font size used, so a
200 px clock font would cost a lot of flash and PSRAM. Single-colour PNGs are
stored as 1-byte alpha maps instead (about 20 KB each).

Slint's software renderer scales images without smoothing, so each board gets
digits at its own pixel size (UI size x the board's UI_SCALE) and they are
drawn 1:1:
    ui/board/esp32s3/clock/  148x167 px (UI scale 1)
    ui/board/esp32p4/clock/  185x209 px (UI scale 1.25)

Run after changing the font or size:
    uv run --with pillow scripts/gen_clock_digits.py
"""
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parent.parent
FONT = ROOT / "ui/fonts/Inter-SemiBold.ttf"
SIZE = 210  # font px at UI scale 1; digits come out about 150 px tall
MARGIN = 4
BOARDS = {"esp32s3": 1.0, "esp32p4": 1.25}


def measure(font: ImageFont.FreeTypeFont, margin: int) -> tuple[dict, int, int, int]:
    boxes = {d: font.getbbox(str(d)) for d in range(10)}
    top = min(b[1] for b in boxes.values())
    bottom = max(b[3] for b in boxes.values())
    width = max(b[2] - b[0] for b in boxes.values()) + 2 * margin
    return boxes, top, width, bottom - top + 2 * margin


_, _, base_w, base_h = measure(ImageFont.truetype(str(FONT), SIZE), MARGIN)
for board, scale in BOARDS.items():
    font = ImageFont.truetype(str(FONT), round(SIZE * scale))
    margin = round(MARGIN * scale)
    boxes, top, w, h = measure(font, margin)
    # Exactly the UI size times the scale, so nothing is resampled on screen.
    width, height = round(base_w * scale), round(base_h * scale)
    out = ROOT / "ui/board" / board / "clock"
    out.mkdir(parents=True, exist_ok=True)
    for d, (x0, _, x1, _) in boxes.items():
        img = Image.new("RGBA", (width, height), (255, 255, 255, 0))
        draw = ImageDraw.Draw(img)
        # Same baseline for every digit, centred both ways in the canvas.
        x = (width - (x1 - x0)) // 2 - x0
        y = (height - h) // 2 + margin - top
        draw.text((x, y), str(d), font=font, fill=(255, 255, 255, 255))
        img.save(out / f"{d}.png", optimize=True)
    print(f"{board}: {width}x{height} px digits in {out} (UI {base_w}x{base_h})")
