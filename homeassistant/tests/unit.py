"""Unit tests for the photo frame and the "at a glance" rules.

    uv run homeassistant/tests/unit.py

They need Home Assistant and Pillow, so on the host this script runs itself
inside the dev container (`esp32-screen-ha`, or ESP32_SCREEN_HA_NAME), where
the integration is mounted at /config/custom_components/esp32_screen.
Prints PASS/FAIL per check and exits non-zero on any failure.
"""

from __future__ import annotations

import asyncio
import io
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
from typing import Any

try:
    import homeassistant  # noqa: F401
    import PIL  # noqa: F401
except ImportError:
    # On the host: run this file inside the dev container.
    container = os.environ.get("ESP32_SCREEN_HA_NAME", "esp32-screen-ha")
    with open(__file__, "rb") as source:
        sys.exit(subprocess.run(["docker", "exec", "-i", container, "python3", "-"], stdin=source, check=False).returncode)

sys.path.insert(0, "/config")

from PIL import Image

from custom_components.esp32_screen import photos
from custom_components.esp32_screen.attention import (
    DEFAULT_STATES,
    attention_icon,
    attention_text,
    needs_condition,
    rule_active,
)
from custom_components.esp32_screen.const import ATTENTION_ICONS
from custom_components.esp32_screen.photos import (
    PhotoFolderError,
    PhotoLibrary,
    render_photo,
    resolve_photo_folder,
    scan_folder,
)
from homeassistant.core import State

RESULTS: list[tuple[bool, str, str]] = []


def check(ok: bool, name: str, detail: str = "") -> bool:
    RESULTS.append((bool(ok), name, detail))
    print(f"{'PASS' if ok else 'FAIL'}  {name}" + (f"  ({detail})" if detail else ""), flush=True)
    return bool(ok)


def jpeg_info(data: bytes) -> tuple[str, int, int]:
    """(SOF marker, width, height) of a JPEG: SOF0 = baseline, SOF2 = progressive."""
    i = 2
    while i < len(data) - 9:
        if data[i] != 0xFF:
            i += 1
            continue
        marker = data[i + 1]
        length = int.from_bytes(data[i + 2:i + 4], "big")
        if marker in (0xC0, 0xC1, 0xC2):
            return f"SOF{marker - 0xC0}", int.from_bytes(data[i + 7:i + 9], "big"), int.from_bytes(data[i + 5:i + 7], "big")
        i += 2 + length
    return "?", 0, 0


# ---- attention rules --------------------------------------------------------------


def st(entity_id: str, state: str, device_class: str | None = None) -> State:
    return State(entity_id, state, {"device_class": device_class} if device_class else {})


def rule_tests() -> None:
    cases: list[tuple[dict[str, Any], State | None, bool, str]] = [
        ({"entity_id": "binary_sensor.door"}, st("binary_sensor.door", "on", "door"), True, "binary_sensor on (default)"),
        ({"entity_id": "binary_sensor.door"}, st("binary_sensor.door", "off", "door"), False, "binary_sensor off"),
        ({"entity_id": "binary_sensor.door"}, st("binary_sensor.door", "unavailable"), False, "unavailable never counts"),
        ({"entity_id": "binary_sensor.door", "states": ["unavailable"]}, st("binary_sensor.door", "unavailable"), False,
         "unavailable never counts, even when listed"),
        ({"entity_id": "binary_sensor.door"}, st("binary_sensor.door", "unknown"), False, "unknown never counts"),
        ({"entity_id": "binary_sensor.door"}, None, False, "missing entity"),
        ({"entity_id": "cover.garage"}, st("cover.garage", "open", "garage"), True, "cover open"),
        ({"entity_id": "cover.garage"}, st("cover.garage", "opening", "garage"), True, "cover opening"),
        ({"entity_id": "cover.garage"}, st("cover.garage", "closed", "garage"), False, "cover closed"),
        ({"entity_id": "cover.garage"}, st("cover.garage", "closing", "garage"), False, "cover closing"),
        ({"entity_id": "lock.door"}, st("lock.door", "unlocked"), True, "lock unlocked"),
        ({"entity_id": "lock.door"}, st("lock.door", "jammed"), True, "lock jammed"),
        ({"entity_id": "lock.door"}, st("lock.door", "locked"), False, "lock locked"),
        ({"entity_id": "light.x"}, st("light.x", "on"), True, "light on"),
        ({"entity_id": "switch.x"}, st("switch.x", "off"), False, "switch off"),
        ({"entity_id": "fan.x"}, st("fan.x", "on"), True, "fan on"),
        ({"entity_id": "input_boolean.x"}, st("input_boolean.x", "on"), True, "input_boolean on"),
        ({"entity_id": "media_player.tv"}, st("media_player.tv", "playing"), True, "media playing"),
        ({"entity_id": "media_player.tv"}, st("media_player.tv", "paused"), False, "media paused"),
        ({"entity_id": "alarm_control_panel.a"}, st("alarm_control_panel.a", "triggered"), True, "alarm triggered"),
        ({"entity_id": "alarm_control_panel.a"}, st("alarm_control_panel.a", "armed_away"), False, "alarm armed"),
        ({"entity_id": "sensor.t"}, st("sensor.t", "40"), False, "sensor without a condition: never"),
        ({"entity_id": "person.a"}, st("person.a", "home"), False, "person without a condition: never"),
        ({"entity_id": "sensor.b", "below": 20}, st("sensor.b", "12", "battery"), True, "below 20: 12"),
        ({"entity_id": "sensor.b", "below": 20}, st("sensor.b", "20", "battery"), False, "below 20: 20 (strict)"),
        ({"entity_id": "sensor.b", "below": 20}, st("sensor.b", "abc"), False, "below: text state"),
        ({"entity_id": "sensor.t", "above": 30.5}, st("sensor.t", "30.6"), True, "above 30.5: 30.6"),
        ({"entity_id": "sensor.t", "above": 30.5, "below": 5}, st("sensor.t", "4"), True, "below or above: below"),
        ({"entity_id": "sensor.t", "above": 30.5, "below": 5}, st("sensor.t", "20"), False, "below or above: between"),
        ({"entity_id": "person.a", "states": ["not_home"]}, st("person.a", "not_home"), True, "states match"),
        ({"entity_id": "person.a", "states": ["Not_Home "]}, st("person.a", "not_home"), True, "states: case and spaces"),
        ({"entity_id": "person.a", "states": ["not_home"]}, st("person.a", "home"), False, "states no match"),
        ({"entity_id": "binary_sensor.d", "states": ["off"]}, st("binary_sensor.d", "on"), False, "states replace the default"),
        ({"entity_id": "binary_sensor.d", "states": ["off"]}, st("binary_sensor.d", "off"), True, "states replace the default (match)"),
        ({"entity_id": "sensor.m", "states": ["error"], "below": 0}, st("sensor.m", "error"), True, "states or threshold: state"),
        ({"entity_id": "sensor.m", "states": ["error"], "below": 0}, st("sensor.m", "-1"), True, "states or threshold: number"),
    ]
    for rule, state, expect, name in cases:
        got = rule_active(rule, state)
        check(got is expect, f"rule: {name}", f"got {got}")
    check(not needs_condition("cover.x") and not needs_condition("binary_sensor.x") and needs_condition("sensor.x")
          and needs_condition("person.x"), "rules for domains without a default need states or a threshold")
    check(set(DEFAULT_STATES) >= {"binary_sensor", "cover", "lock", "light", "switch", "fan", "input_boolean",
                                  "media_player", "alarm_control_panel"}, "domain defaults cover the documented domains")


def icon_tests() -> None:
    cases = [
        ("binary_sensor", "door", "door"), ("binary_sensor", "opening", "door"), ("binary_sensor", "window", "window"),
        ("binary_sensor", "garage_door", "garage"), ("binary_sensor", "motion", "motion"),
        ("binary_sensor", "occupancy", "motion"), ("binary_sensor", "presence", "motion"),
        ("binary_sensor", "moisture", "water"), ("binary_sensor", "smoke", "smoke"), ("binary_sensor", "gas", "smoke"),
        ("binary_sensor", "carbon_monoxide", "smoke"), ("binary_sensor", "battery", "battery"),
        ("binary_sensor", "heat", "thermometer"), ("binary_sensor", "cold", "thermometer"),
        ("binary_sensor", None, "alert"), ("binary_sensor", "problem", "alert"),
        ("cover", "garage", "garage"), ("cover", "gate", "garage"), ("cover", "window", "window"),
        ("cover", "blind", "window"), ("cover", "shutter", "window"), ("cover", "curtain", "window"),
        ("cover", "shade", "window"), ("cover", "awning", "window"), ("cover", None, "garage"),
        ("lock", None, "lock"), ("light", None, "light"), ("switch", "outlet", "power"), ("fan", None, "power"),
        ("input_boolean", None, "power"), ("media_player", "tv", "power"),
        ("sensor", "battery", "battery"), ("sensor", "temperature", "thermometer"), ("sensor", "humidity", "alert"),
        ("alarm_control_panel", None, "alert"), ("person", None, "alert"),
    ]
    bad = [(d, c, attention_icon(d, c), want) for d, c, want in cases if attention_icon(d, c) != want]
    check(not bad, f"icons by domain and device class ({len(cases)} cases)", str(bad))
    every = {attention_icon(d, c) for d in ("binary_sensor", "cover", "sensor", "lock", "x")
             for c in (None, "door", "window", "battery", "smoke", "x")}
    check(every <= set(ATTENTION_ICONS), "icons are always one of the firmware's glyphs", str(every - set(ATTENTION_ICONS)))
    check(attention_text("Front door", "Open") == "Front door · Open" and attention_text("X", "") == "X",
          "text is '<label> · <state>'")


# ---- photo folder -----------------------------------------------------------------


def folder_tests(tmp: Path) -> None:
    local = tmp / "media"
    nas = tmp / "nas"
    outside = tmp / "outside"
    for d in (local / "photos" / "family", nas / "pics", outside):
        d.mkdir(parents=True)
    (local / "escape").symlink_to(outside)
    (local / "alias").symlink_to(local / "photos")
    dirs = {"local": str(local), "nas": str(nas)}

    def ok(value: str, expect: Path | None, name: str) -> None:
        try:
            got = resolve_photo_folder(dirs, value)
        except PhotoFolderError as err:
            check(False, f"folder: {name}", f"rejected: {err}")
            return
        check(got == (expect.resolve() if expect else None), f"folder: {name}", str(got))

    def bad(value: str, name: str) -> None:
        try:
            got = resolve_photo_folder(dirs, value)
        except PhotoFolderError as err:
            check(True, f"folder rejected: {name}", str(err))
            return
        check(False, f"folder rejected: {name}", f"accepted as {got}")

    ok("", None, "empty = no folder")
    ok("photos", local / "photos", "relative to local")
    ok(" photos/family/ ", local / "photos" / "family", "subfolder, spaces trimmed")
    ok("nas:pics", nas / "pics", "<source>:<path>")
    ok("local:photos", local / "photos", "local:<path>")
    ok("nas:", nas, "a whole media folder")
    ok(f"{local}/photos", local / "photos", "absolute path inside a media folder")
    ok("new/not-yet", local / "new" / "not-yet", "a folder that does not exist yet")
    ok("alias/family", local / "photos" / "family", "a symlink that stays inside")
    bad("../outside", "..")
    bad("photos/../../outside", "nested ..")
    bad("/etc", "absolute path outside")
    bad(f"{local}/../outside", "absolute path with ..")
    bad("escape", "symlink leading out")
    bad("escape/sub", "below a symlink leading out")
    bad("usb:photos", "unknown media folder")
    bad("nas:/etc", "absolute path after the source")
    bad("x" * 300, "too long")
    try:
        resolve_photo_folder({}, "photos")
        check(False, "folder rejected: no media folders")
    except PhotoFolderError:
        check(True, "folder rejected: no media folders")


def scan_tests(tmp: Path) -> None:
    root = tmp / "scan"
    (root / "sub" / "deeper").mkdir(parents=True)
    (root / ".hidden").mkdir()
    for name in ("a.JPG", "b.jpeg", "c.png", "d.WebP", "sub/e.jpg", "sub/deeper/f.png", ".hidden/g.jpg",
                 ".h.jpg", "x.gif", "notes.txt", "raw.heic"):
        (root / name).write_bytes(b"x")
    outside = tmp / "elsewhere.jpg"
    outside.write_bytes(b"x")
    (root / "link-out.jpg").symlink_to(outside)
    (root / "link-in.jpg").symlink_to(root / "a.JPG")
    names = sorted(p.relative_to(root.resolve()).as_posix() for p in scan_folder(root))
    want = ["a.JPG", "b.jpeg", "c.png", "d.WebP", "link-in.jpg", "sub/deeper/f.png", "sub/e.jpg"]
    check(names == want, "scan: jpg/jpeg/png/webp in any case, recursive; no hidden, other types or links out", str(names))
    check(len(scan_folder(root, limit=3)) == 3, "scan stops at the limit")
    check(scan_folder(tmp / "missing") == [] and scan_folder(None) == [], "scan: missing folder -> none")


# ---- rendering --------------------------------------------------------------------


def make_image(path: Path, size: tuple[int, int], color: tuple[int, ...], mode: str = "RGB", fmt: str | None = None,
               **save: Any) -> Path:
    Image.new(mode, size, color).save(path, fmt, **save)
    return path


def render_tests(tmp: Path) -> None:
    def info(data: bytes) -> tuple[str, int, int, Image.Image]:
        sof, w, h = jpeg_info(data)
        return sof, w, h, Image.open(io.BytesIO(data)).convert("RGB")

    red = (230, 20, 20)
    cases = [
        ("big landscape JPEG", make_image(tmp / "big.jpg", (4000, 3000), red, quality=90), (800, 480)),
        ("progressive JPEG", make_image(tmp / "prog.jpg", (1600, 900), red, progressive=True), (800, 480)),
        ("portrait PNG", make_image(tmp / "portrait.png", (900, 1600), red), (800, 480)),
        ("WebP", make_image(tmp / "w.webp", (640, 480), red), (1024, 600)),
        ("grey JPEG on a portrait screen", make_image(tmp / "grey.jpg", (1000, 700), (128,), mode="L"), (480, 800)),
        ("palette PNG", make_image(tmp / "pal.png", (300, 200), 3, mode="P"), (16, 16)),
        ("largest size", make_image(tmp / "max.png", (200, 120), red), (1280, 800)),
    ]
    for name, path, (w, h) in cases:
        started = time.monotonic()
        data = render_photo(path, w, h)
        secs = time.monotonic() - started
        sof, jw, jh, img = info(data)
        check(sof == "SOF0" and (jw, jh) == (w, h) and img.size == (w, h),
              f"render {name}: baseline JPEG of exactly {w}x{h}", f"{sof} {jw}x{jh} {len(data)} bytes {secs:.2f} s")

    # Close aspect ratio (1000x700 on 800x480): fills the screen, cropped.
    img = info(render_photo(tmp / "grey.jpg", 800, 480))[3]
    corner, center = img.getpixel((2, 2)), img.getpixel((400, 240))
    check(abs(corner[0] - 128) <= 6 and abs(center[0] - 128) <= 6, "close aspect ratio: fills and crops (no border)",
          f"corner {corner} center {center}")
    # Portrait on landscape: whole photo over a dark blurred copy.
    img = info(render_photo(tmp / "portrait.png", 800, 480))[3]
    edge, center = img.getpixel((10, 240)), img.getpixel((400, 240))
    check(center[0] > 200 and 80 < edge[0] < 130, "portrait on landscape: whole photo over a darkened copy (~45 %)",
          f"edge {edge} center {center}")
    # Left half black, right half white: the fitted photo keeps both halves.
    half = Image.new("RGB", (600, 1200), (0, 0, 0))
    half.paste((255, 255, 255), (300, 0, 600, 1200))
    half.save(tmp / "half.png")
    img = info(render_photo(tmp / "half.png", 800, 480))[3]
    left, right = img.getpixel((400 - 60, 240)), img.getpixel((400 + 60, 240))
    check(left[0] < 30 and right[0] > 225, "fitted photo is centred and not cropped", f"{left} {right}")

    # EXIF orientation 6: stored 600x400 (landscape), shown 400x600 (portrait),
    # so it is fitted, not cropped.
    exif = Image.Exif()
    exif[0x0112] = 6
    make_image(tmp / "rotated.jpg", (600, 400), red, exif=exif.tobytes())
    img = info(render_photo(tmp / "rotated.jpg", 800, 480))[3]
    edge, center = img.getpixel((10, 240)), img.getpixel((400, 240))
    check(center[0] > 200 and edge[0] < 130, "EXIF orientation is applied (rotated photo is fitted)", f"edge {edge} center {center}")

    # Transparency goes on black.
    make_image(tmp / "clear.png", (800, 480), (255, 255, 255, 0), mode="RGBA")
    img = info(render_photo(tmp / "clear.png", 800, 480))[3]
    check(max(img.getpixel((400, 240))) < 10, "transparent PNG -> black", str(img.getpixel((400, 240))))

    (tmp / "broken.jpg").write_bytes(b"not a picture")
    try:
        render_photo(tmp / "broken.jpg", 800, 480)
        check(False, "a broken file raises")
    except Exception as err:  # PIL raises UnidentifiedImageError (an OSError)
        check(True, "a broken file raises", type(err).__name__)


class FakeConfig:
    def __init__(self, media_dirs: dict[str, str]) -> None:
        self.media_dirs = media_dirs


class FakeHass:
    def __init__(self, media_dirs: dict[str, str]) -> None:
        self.config = FakeConfig(media_dirs)

    async def async_add_executor_job(self, func: Any, *args: Any) -> Any:
        return func(*args)


def library_tests(tmp: Path) -> None:
    media = tmp / "lib"
    (media / "set").mkdir(parents=True)
    for name in ("a.png", "b.png", "c.png", "bad.jpg"):
        (media / "set" / name).write_bytes(b"x")
    (media / "other").mkdir()
    (media / "other" / "z.png").write_bytes(b"x")

    def fake_render(path: Path, width: int, height: int) -> bytes:
        if path.name.startswith("bad"):
            raise OSError("cannot identify image file")
        return f"{path.name} {width}x{height}".encode()

    real = photos.render_photo
    photos.render_photo = fake_render
    try:
        lib = PhotoLibrary(FakeHass({"local": str(media)}))  # type: ignore[arg-type]

        async def run() -> None:
            check(await lib.async_count("set") == 4 and await lib.async_count("") == 0
                  and await lib.async_count("missing") == 0 and await lib.async_count("../x") == 0,
                  "count: 4 in the folder, 0 for none, missing or invalid")
            shown = []
            for _ in range(9):
                data, pos, count = await lib.async_next("S1", "set", 800, 480)  # type: ignore[misc]
                shown.append((data.decode().split()[0], pos, count))
            names = [s[0] for s in shown]
            check(all(sorted(names[i:i + 3]) == ["a.png", "b.png", "c.png"] for i in (0, 3, 6)),
                  "each pass shows every readable photo once (broken ones skipped)", str(shown))
            check(all(s[2] == 4 and 1 <= s[1] <= 4 for s in shown), "position i/N within the pass", str(shown))
            check(shown[0][0] != "" and b"800x480" in (await lib.async_next("S1", "set", 800, 480))[0],  # type: ignore[index]
                  "requested size is passed on")
            other = await lib.async_next("S2", "other", 100, 100)
            check(other is not None and other[0].startswith(b"z.png") and other[1:] == (1, 1),
                  "screens have their own order and folder", str(other))
            data, _, _ = await lib.async_next("S1", "other", 10, 10)  # type: ignore[misc]
            check(data.startswith(b"z.png"), "a new folder for a screen starts a new order")
            check(await lib.async_next("S1", "", 10, 10) is None, "no folder -> no photo")
            (media / "bad").mkdir()
            (media / "bad" / "bad1.jpg").write_bytes(b"x")
            check(await lib.async_next("S3", "bad", 10, 10) is None, "only unreadable files -> no photo")

        asyncio.run(run())
    finally:
        photos.render_photo = real

    check(photos._dimension("5000", (16, 1280)) == 1280 and photos._dimension("1", (16, 800)) == 16
          and photos._dimension("480", (16, 800)) == 480, "w/h are clamped")
    for raw in (None, "abc", "1.5", ""):
        try:
            photos._dimension(raw, (16, 1280))
            check(False, f"w/h {raw!r} rejected")
        except ValueError:
            check(True, f"w/h {raw!r} rejected")


def main() -> int:
    rule_tests()
    icon_tests()
    with tempfile.TemporaryDirectory() as tmp_name:
        tmp = Path(tmp_name)
        folder_tests(tmp)
        scan_tests(tmp)
        render_tests(tmp)
        library_tests(tmp)
    failed = [r for r in RESULTS if not r[0]]
    print(f"\n{len(RESULTS) - len(failed)}/{len(RESULTS)} checks passed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
