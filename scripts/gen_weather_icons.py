#!/usr/bin/env python3
"""Render the colour weather icons (Meteocons, MIT, ui/weather-svg/) to PNGs
at each board's pixel size, for the screensaver's "colour" weather icons.

Slint's software renderer scales images without smoothing, so each board
gets its own set, drawn 1:1 (UI size x the board's UI_SCALE):
    ui/board/<chip>/weather/<icon>-small.png   clock line   (SMALL UI px)
    ui/board/<chip>/weather/<icon>-large.png   forecast     (LARGE UI px)

    uv run --with playwright scripts/gen_weather_icons.py
(needs Playwright's Chromium: uv run --with playwright playwright install chromium)
"""
from pathlib import Path

from playwright.sync_api import sync_playwright

ROOT = Path(__file__).resolve().parent.parent
SVG = ROOT / "ui/weather-svg"
SMALL, LARGE = 36, 64  # UI px
BOARDS = {"esp32s3": 1.0, "esp32p4": 1.25}

with sync_playwright() as p:
    browser = p.chromium.launch()
    page = browser.new_page()
    for board, scale in BOARDS.items():
        out = ROOT / "ui/board" / board / "weather"
        out.mkdir(parents=True, exist_ok=True)
        for svg in sorted(SVG.glob("*.svg")):
            for name, ui in (("small", SMALL), ("large", LARGE)):
                px = round(ui * scale)
                page.set_content(
                    f'<html><body style="margin:0;background:transparent">'
                    f'<img id="i" src="data:image/svg+xml;base64,'
                    f'{__import__("base64").b64encode(svg.read_bytes()).decode()}" '
                    f'width="{px}" height="{px}" style="display:block"></body></html>'
                )
                page.locator("#i").screenshot(
                    path=str(out / f"{svg.stem}-{name}.png"), omit_background=True
                )
        print(f"{board}: {len(list(SVG.glob('*.svg')))} icons at {round(SMALL*scale)} and {round(LARGE*scale)} px in {out}")
    browser.close()
