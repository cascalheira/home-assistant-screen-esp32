"""Firmware images for over-the-air updates (docs/protocol.md, section 5)."""

from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass
from functools import cmp_to_key
import hashlib
import logging
from pathlib import Path
import struct

from aiohttp import hdrs, web
from awesomeversion import AwesomeVersion, AwesomeVersionCompareException

from homeassistant.components.http import HomeAssistantView
from homeassistant.core import CALLBACK_TYPE, HomeAssistant, callback
from homeassistant.helpers.http import KEY_HASS

from .const import (
    DOMAIN,
    FIRMWARE_DIR,
    FIRMWARE_PROJECT,
    FIRMWARE_URL,
)

_LOGGER = logging.getLogger(__name__)

# esp_image_header_t (24 bytes) + first esp_image_segment_header_t (8 bytes),
# then esp_app_desc_t.
APP_DESC_OFFSET = 32
APP_DESC_MAGIC = 0xABCD5432
VERSION_OFFSET = 48
PROJECT_OFFSET = 80
FIELD_LEN = 32
HEADER_LEN = PROJECT_OFFSET + FIELD_LEN


@dataclass(frozen=True)
class FirmwareImage:
    """A valid firmware file."""

    path: Path
    version: str
    sha256: str
    size: int

    @property
    def url(self) -> str:
        """Download path for the screen."""
        return f"{FIRMWARE_URL}/{self.sha256}.bin"


def _cstr(raw: bytes) -> str:
    return raw.split(b"\0", 1)[0].decode("utf-8", errors="replace").strip()


def parse_image(path: Path) -> tuple[FirmwareImage | None, str]:
    """Read and check one image. Returns (image, reason when invalid)."""
    with path.open("rb") as file:
        header = file.read(HEADER_LEN)
        if len(header) < HEADER_LEN:
            return None, "too short for an app descriptor"
        (magic,) = struct.unpack_from("<I", header, APP_DESC_OFFSET)
        if magic != APP_DESC_MAGIC:
            return None, f"no app descriptor (magic 0x{magic:08X})"
        project = _cstr(header[PROJECT_OFFSET : PROJECT_OFFSET + FIELD_LEN])
        if project != FIRMWARE_PROJECT:
            return None, f"project is {project!r}, not {FIRMWARE_PROJECT!r}"
        version = _cstr(header[VERSION_OFFSET : VERSION_OFFSET + FIELD_LEN])
        if not version:
            return None, "empty version"
        digest = hashlib.sha256(header)
        size = len(header)
        while chunk := file.read(1 << 16):
            digest.update(chunk)
            size += len(chunk)
    return FirmwareImage(path, version, digest.hexdigest(), size), ""


def _compare(a: FirmwareImage, b: FirmwareImage) -> int:
    try:
        va, vb = AwesomeVersion(a.version), AwesomeVersion(b.version)
        if va == vb:
            return 0
        return 1 if va > vb else -1
    except AwesomeVersionCompareException:
        return (a.version > b.version) - (a.version < b.version)


class FirmwareStore:
    """Scans the firmware folder and keeps the valid images."""

    def __init__(self, hass: HomeAssistant) -> None:
        """Initialize."""
        self.hass = hass
        self.folder = Path(hass.config.path(FIRMWARE_DIR))
        self.images: list[FirmwareImage] = []
        self.latest: FirmwareImage | None = None
        # path -> (mtime_ns, size, image or None)
        self._cache: dict[Path, tuple[int, int, FirmwareImage | None]] = {}
        self._listeners: set[Callable[[], None]] = set()

    def _scan(self) -> list[FirmwareImage]:
        self.folder.mkdir(parents=True, exist_ok=True)
        seen: dict[Path, tuple[int, int, FirmwareImage | None]] = {}
        for path in sorted(self.folder.glob("*.bin")):
            try:
                stat = path.stat()
                key = (stat.st_mtime_ns, stat.st_size)
                cached = self._cache.get(path)
                if cached is not None and cached[:2] == key:
                    seen[path] = cached
                    continue
                image, reason = parse_image(path)
            except OSError as err:
                _LOGGER.warning("Cannot read firmware file %s: %s", path, err)
                continue
            if image is None:
                _LOGGER.warning("Ignoring firmware file %s: %s", path.name, reason)
            else:
                _LOGGER.debug(
                    "Firmware %s: version %s, %d bytes", path.name, image.version, image.size
                )
            seen[path] = (*key, image)
        self._cache = seen
        return [entry[2] for entry in seen.values() if entry[2] is not None]

    async def async_scan(self) -> None:
        """Rescan the folder (in the executor) and notify on changes."""
        images = await self.hass.async_add_executor_job(self._scan)
        latest = max(images, key=cmp_to_key(_compare)) if images else None
        changed = images != self.images or latest != self.latest
        self.images = images
        self.latest = latest
        if changed:
            _LOGGER.info(
                "Firmware folder %s: latest %s",
                self.folder,
                f"{latest.version} ({latest.path.name})" if latest else "none",
            )
            for listener in list(self._listeners):
                listener()

    def by_sha256(self, sha256: str) -> FirmwareImage | None:
        """Return the valid image with this hash."""
        return next((img for img in self.images if img.sha256 == sha256), None)

    @callback
    def async_add_listener(self, listener: Callable[[], None]) -> CALLBACK_TYPE:
        """Call `listener` when the set of images changes."""
        self._listeners.add(listener)

        @callback
        def remove() -> None:
            self._listeners.discard(listener)

        return remove


class FirmwareDownloadView(HomeAssistantView):
    """Serves firmware images to screens (any authenticated user)."""

    url = FIRMWARE_URL + "/{sha256}.bin"
    name = "api:esp32_screen:firmware"
    requires_auth = True

    async def get(self, request: web.Request, sha256: str) -> web.StreamResponse:
        """Stream one image."""
        hass: HomeAssistant = request.app[KEY_HASS]
        manager = hass.data.get(DOMAIN)
        image = manager.firmware.by_sha256(sha256.lower()) if manager else None
        if image is None:
            return web.Response(status=404, text="Unknown firmware")
        return web.FileResponse(
            image.path,
            headers={
                hdrs.CONTENT_TYPE: "application/octet-stream",
                hdrs.CONTENT_DISPOSITION: f'attachment; filename="{image.path.name}"',
                hdrs.CACHE_CONTROL: "no-store",
            },
        )

