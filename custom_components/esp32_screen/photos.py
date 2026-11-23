"""Photo frame: screensaver photos from a media folder (docs/protocol.md, section 7).

A screen's `photo_folder` setting is a folder in one of Home Assistant's media
folders (`homeassistant: media_dirs:`, by default `local` = /media):

- `photos/family`: relative to the `local` media folder (the first one if
  there is no `local`);
- `nas:photos/family`: relative to the media folder named `nas`;
- `/media/photos/family`: an absolute path inside a media folder.

Paths that leave the media folder (`..`, symlinks pointing elsewhere) are
rejected. The screen fetches one photo at a time, already scaled and cropped to
its size, from PhotoView.
"""

from __future__ import annotations

from dataclasses import dataclass, field
import io
import logging
import os
from pathlib import Path, PurePosixPath
import random
import time

from aiohttp import hdrs, web

from homeassistant.components.http import HomeAssistantView
from homeassistant.core import HomeAssistant
from homeassistant.helpers.http import KEY_HASS

from .const import (
    DOMAIN,
    MAX_PHOTO_FOLDER_LEN,
    PHOTO_EXTENSIONS,
    PHOTO_HEIGHT,
    PHOTO_MAX_FILES,
    PHOTO_RESCAN,
    PHOTO_URL,
    PHOTO_WIDTH,
)

_LOGGER = logging.getLogger(__name__)

JPEG_QUALITY = 85
# Fit the whole photo (over a blurred copy) when its aspect ratio differs from
# the screen's by more than this factor; otherwise fill the screen and crop.
CROP_TOLERANCE = 1.25
BLUR_RADIUS = 30
BACKGROUND_BRIGHTNESS = 0.45
# Files tried per request before giving up (unreadable images are skipped).
MAX_SKIPS = 20


class PhotoFolderError(ValueError):
    """The photo folder setting is not a folder inside a media folder."""


def resolve_photo_folder(media_dirs: dict[str, str], value: str) -> Path | None:
    """The folder a `photo_folder` setting names, or None for "".

    Raises PhotoFolderError for paths outside the media folders. The folder
    need not exist (yet). Blocking I/O (symlinks are resolved): run in the
    executor.
    """
    value = value.strip()
    if not value:
        return None
    if len(value) > MAX_PHOTO_FOLDER_LEN or "\0" in value:
        raise PhotoFolderError("The photo folder name is too long or invalid")
    if not media_dirs:
        raise PhotoFolderError("Home Assistant has no media folder")
    if value.startswith("/"):
        target = Path(value)
        bases = [Path(d) for d in media_dirs.values()]
        base = next((b for b in bases if _inside(target, b)), None)
        if base is None:
            raise PhotoFolderError(
                f"{value} is not inside a media folder ({', '.join(media_dirs.values())})"
            )
        relative = PurePosixPath(os.path.relpath(target, base))
    else:
        source, sep, rest = value.partition(":")
        if sep and source in media_dirs:
            base = Path(media_dirs[source])
            relative = PurePosixPath(rest)
        elif sep and "/" not in source:
            raise PhotoFolderError(
                f"Unknown media folder '{source}' (known: {', '.join(media_dirs)})"
            )
        else:
            base = Path(media_dirs.get("local") or next(iter(media_dirs.values())))
            relative = PurePosixPath(value)
    if relative.is_absolute() or ".." in relative.parts:
        raise PhotoFolderError("The photo folder must stay inside the media folder")
    root = base.resolve()
    folder = (root / relative).resolve()
    if not folder.is_relative_to(root):
        raise PhotoFolderError("The photo folder must stay inside the media folder")
    return folder


def _inside(path: Path, base: Path) -> bool:
    try:
        return Path(os.path.normpath(path)).is_relative_to(os.path.normpath(base))
    except ValueError:
        return False


def scan_folder(folder: Path | None, limit: int = PHOTO_MAX_FILES) -> list[Path]:
    """Images (jpg, jpeg, png, webp) in `folder` and its subfolders, sorted.

    At most `limit`. Symlinks that lead out of the folder and hidden files are
    left out. A missing folder has none. Blocking I/O: run in the executor.
    """
    if folder is None or not folder.is_dir():
        return []
    root = folder.resolve()
    found: list[Path] = []
    pending = [root]
    while pending and len(found) < limit:
        current = pending.pop()
        try:
            entries = sorted(os.scandir(current), key=lambda e: e.name)
        except OSError:
            continue
        for entry in entries:
            if entry.name.startswith("."):
                continue
            path = Path(entry.path)
            try:
                if entry.is_symlink() and not path.resolve().is_relative_to(root):
                    continue
                if entry.is_dir(follow_symlinks=False):
                    pending.append(path)
                elif entry.name.lower().endswith(PHOTO_EXTENSIONS) and entry.is_file():
                    found.append(path)
            except OSError:
                continue
            if len(found) >= limit:
                break
    return sorted(found)


def render_photo(path: Path, width: int, height: int) -> bytes:
    """`path` as a baseline JPEG of exactly width x height (runs in the executor).

    Photos with about the screen's aspect ratio fill it and are cropped in the
    middle. Others (a portrait photo on a landscape screen) are shown whole,
    over a blurred, darkened copy that fills the screen.
    """
    from PIL import Image, ImageEnhance, ImageFilter, ImageOps  # Pillow ships with HA

    with Image.open(path) as img:
        if img.format == "JPEG":
            # Decode at a reduced scale: much faster for large camera photos.
            orientation = img.getexif().get(0x0112, 1)
            want = (height * 2, width * 2) if orientation in (5, 6, 7, 8) else (width * 2, height * 2)
            img.draft("RGB", want)
        img.load()
        photo = ImageOps.exif_transpose(img)
    if photo.mode in ("RGBA", "LA", "PA") or (photo.mode == "P" and "transparency" in photo.info):
        rgba = photo.convert("RGBA")
        photo = Image.new("RGB", rgba.size, (0, 0, 0))
        photo.paste(rgba, mask=rgba.getchannel("A"))
    elif photo.mode != "RGB":
        photo = photo.convert("RGB")

    ratio = (photo.width / photo.height) / (width / height)
    if 1 / CROP_TOLERANCE <= ratio <= CROP_TOLERANCE:
        out = ImageOps.fit(photo, (width, height), Image.Resampling.LANCZOS)
    else:
        # Blur a quarter-size copy: the same look, much less work.
        small = (max(1, width // 4), max(1, height // 4))
        background = ImageOps.fit(photo, small, Image.Resampling.BILINEAR)
        background = background.filter(ImageFilter.GaussianBlur(BLUR_RADIUS / 4))
        background = ImageEnhance.Brightness(background).enhance(BACKGROUND_BRIGHTNESS)
        out = background.resize((width, height), Image.Resampling.BILINEAR)
        front = ImageOps.contain(photo, (width, height), Image.Resampling.LANCZOS)
        out.paste(front, ((width - front.width) // 2, (height - front.height) // 2))
    buf = io.BytesIO()
    out.save(buf, "JPEG", quality=JPEG_QUALITY, progressive=False, optimize=True)
    return buf.getvalue()


def _render_first(paths: list[Path], width: int, height: int) -> tuple[int, bytes] | None:
    """Render the first readable photo of `paths`: (its index, JPEG)."""
    for index, path in enumerate(paths):
        try:
            return index, render_photo(path, width, height)
        except Exception as err:  # any unreadable file is skipped
            _LOGGER.debug("Skipping photo %s: %s", path, err)
    return None


@dataclass
class _Scan:
    files: list[Path]
    at: float


@dataclass
class _Order:
    """A screen's way through the photos: a shuffled pass, then another."""

    folder: str
    files: list[Path]
    order: list[Path] = field(default_factory=list)
    pos: int = 0


class PhotoLibrary:
    """Scans photo folders and hands each screen its next photo."""

    def __init__(self, hass: HomeAssistant) -> None:
        """Initialize."""
        self.hass = hass
        self._scans: dict[str, _Scan] = {}
        self._orders: dict[str, _Order] = {}

    async def async_files(self, folder: str, max_age: float = PHOTO_RESCAN) -> list[Path]:
        """The images in a `photo_folder`, scanned again when older than `max_age`."""
        folder = folder.strip()
        if not folder:
            return []
        cached = self._scans.get(folder)
        if cached is not None and time.monotonic() - cached.at < max_age:
            return cached.files
        files = await self.hass.async_add_executor_job(self._scan, folder)
        cached = self._scans.get(folder)
        if cached is not None and cached.files == files:
            files = cached.files  # unchanged: screens keep their order
        self._scans[folder] = _Scan(files, time.monotonic())
        return files

    def _scan(self, folder: str) -> list[Path]:
        try:
            path = resolve_photo_folder(self.hass.config.media_dirs, folder)
        except PhotoFolderError as err:
            _LOGGER.warning("Photo folder %s: %s", folder, err)
            return []
        return scan_folder(path)

    async def async_count(self, folder: str, max_age: float = PHOTO_RESCAN) -> int:
        """How many images a `photo_folder` has."""
        return len(await self.async_files(folder, max_age))

    async def async_next(
        self, screen_id: str, folder: str, width: int, height: int
    ) -> tuple[bytes, int, int] | None:
        """The screen's next photo as a JPEG: (data, 1-based position, count)."""
        files = await self.async_files(folder)
        if not files:
            return None
        order = self._orders.get(screen_id)
        if order is None or order.folder != folder or order.files is not files:
            order = _Order(folder, files)
            self._orders[screen_id] = order
        tried = 0
        while tried < min(len(files), MAX_SKIPS):
            if order.pos >= len(order.order):
                order.order = random.sample(files, len(files))
                order.pos = 0
            candidates = order.order[order.pos : order.pos + MAX_SKIPS - tried]
            rendered = await self.hass.async_add_executor_job(
                _render_first, candidates, width, height
            )
            if rendered is not None:
                index, data = rendered
                order.pos += index + 1
                return data, min(order.pos, len(files)), len(files)
            order.pos += len(candidates)
            tried += len(candidates)
        return None

    def forget(self, screen_id: str) -> None:
        """Drop a removed screen's order."""
        self._orders.pop(screen_id, None)


def _dimension(raw: str | None, limits: tuple[int, int]) -> int:
    if raw is None:
        raise ValueError("missing")
    return max(limits[0], min(limits[1], int(raw)))


class PhotoView(HomeAssistantView):
    """Serves each screen its next photo (any authenticated user)."""

    url = PHOTO_URL + "/{screen_id}"
    name = "api:esp32_screen:photo"
    requires_auth = True

    async def get(self, request: web.Request, screen_id: str) -> web.Response:
        """Return the next photo, `w` x `h` pixels."""
        try:
            width = _dimension(request.query.get("w"), PHOTO_WIDTH)
            height = _dimension(request.query.get("h"), PHOTO_HEIGHT)
        except ValueError:
            return web.Response(status=400, text="w and h must be whole numbers")
        hass: HomeAssistant = request.app[KEY_HASS]
        manager = hass.data.get(DOMAIN)
        record = manager.screens.get(screen_id) if manager else None
        if record is None:
            return web.Response(status=404, text="Unknown screen")
        photo = await manager.photos.async_next(
            screen_id, record.settings.get("photo_folder") or "", width, height
        )
        if photo is None:
            return web.Response(status=404, text="No photos")
        data, position, count = photo
        return web.Response(
            body=data,
            headers={
                hdrs.CONTENT_TYPE: "image/jpeg",
                hdrs.CACHE_CONTROL: "no-store",
                "X-Photo-Index": f"{position}/{count}",
            },
        )
