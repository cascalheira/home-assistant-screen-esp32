"""Doorbell: a ring shows the door camera on the screens (docs/protocol.md, section 9).

Each screen's settings name a ring entity (an `event.*` that fires on a ring,
or a `binary_sensor.*` that turns on), a camera, how long to show it, and
optionally an entity the screen can open (gate, lock, ...). On a ring, every
online screen with that ring entity gets a `doorbell` event with a stream
URL. The stream is MJPEG made by ffmpeg from the camera's stream source (RTSP),
small enough for the screen to decode a few frames a second; a camera without
a stream source sends snapshots instead.
"""

from __future__ import annotations

import asyncio
from dataclasses import dataclass
import logging
import secrets
import shutil
import time
from typing import TYPE_CHECKING

from aiohttp import hdrs, web

from homeassistant.components.http import HomeAssistantView
from homeassistant.const import STATE_ON, STATE_UNAVAILABLE, STATE_UNKNOWN
from homeassistant.core import (
    CALLBACK_TYPE,
    Context,
    Event,
    EventStateChangedData,
    HomeAssistant,
    callback,
)
from homeassistant.exceptions import HomeAssistantError
from homeassistant.helpers.event import async_track_state_change_event
from homeassistant.helpers.http import KEY_HASS

from .const import DOMAIN

if TYPE_CHECKING:
    from .manager import ScreenManager

_LOGGER = logging.getLogger(__name__)

DOORBELL_URL = "/api/esp32_screen/doorbell"
# The video fits this box (aspect ratio kept): the screen decodes JPEGs of up
# to 640x400 and holds two frames in memory.
STREAM_MAX_SIZE = (512, 384)
STREAM_FPS = 6
STREAM_QUALITY = 7  # ffmpeg -q:v, 2 (best) - 31
SNAPSHOT_INTERVAL = 0.5  # seconds, for cameras without a stream source
BOUNDARY = "frame"
# A stream may be opened this long after its ring's timeout (slow start).
GRACE = 10

RING_DOMAINS = ("event", "binary_sensor")
ACTION_SERVICES: dict[str, tuple[str, str]] = {
    "lock": ("lock", "unlock"),
    "cover": ("cover", "open_cover"),
    "switch": ("switch", "turn_on"),
    "button": ("button", "press"),
    "input_button": ("input_button", "press"),
    "script": ("script", "turn_on"),
}


@dataclass
class Ring:
    """One ring shown on one screen."""

    ring_id: str
    screen_id: str
    camera: str
    expires: float


def is_ring(event: Event[EventStateChangedData]) -> bool:
    """Whether a state change of a ring entity is a ring.

    `event.*`: its state (the time it last fired) changed. A state restored at
    startup (from unavailable) is not a ring. `binary_sensor.*`: off -> on.
    """
    old, new = event.data["old_state"], event.data["new_state"]
    if new is None or old is None or new.state in (STATE_UNAVAILABLE, STATE_UNKNOWN):
        return False
    if old.state == STATE_UNAVAILABLE:
        return False
    if new.domain == "binary_sensor":
        return new.state == STATE_ON and old.state != STATE_ON
    return new.state != old.state


class Doorbells:
    """Ring tracking and the rings currently shown."""

    def __init__(self, manager: ScreenManager) -> None:
        """Initialize."""
        self.manager = manager
        self.hass: HomeAssistant = manager.hass
        self.rings: dict[str, Ring] = {}
        self._unsub: CALLBACK_TYPE | None = None

    @callback
    def async_track(self) -> None:
        """(Re)track the ring entities of all screens. Call after settings change."""
        self.async_stop()
        entities = sorted(
            {
                rec.settings.get("doorbell_entity")
                for rec in self.manager.screens.values()
                if rec.settings.get("doorbell_entity")
            }
        )
        if entities:
            self._unsub = async_track_state_change_event(self.hass, entities, self._changed)

    @callback
    def async_stop(self) -> None:
        """Stop tracking."""
        if self._unsub is not None:
            self._unsub()
            self._unsub = None

    @callback
    def _changed(self, event: Event[EventStateChangedData]) -> None:
        if not is_ring(event):
            return
        entity_id = event.data["entity_id"]
        screens = [
            sid
            for sid, rec in self.manager.screens.items()
            if rec.settings.get("doorbell_entity") == entity_id
        ]
        _LOGGER.debug("%s rang; screens %s", entity_id, screens)
        for screen_id in screens:
            self.async_ring(screen_id)

    @callback
    def async_ring(self, screen_id: str) -> str | None:
        """Show the doorbell on a screen. Returns the ring id, None when offline."""
        record = self.manager.screens.get(screen_id)
        if record is None or not self.manager.is_online(screen_id):
            return None
        self._purge()
        settings = record.settings
        camera = settings.get("doorbell_camera") or ""
        timeout = int(settings.get("doorbell_timeout") or 60)
        ring_id = secrets.token_hex(8)
        self.rings[ring_id] = Ring(ring_id, screen_id, camera, time.monotonic() + timeout)
        title = "Doorbell"
        if camera and (state := self.hass.states.get(camera)) is not None:
            title = state.attributes.get("friendly_name") or title
        action = ""
        if (action_entity := settings.get("doorbell_action")) and (
            state := self.hass.states.get(action_entity)
        ) is not None:
            action = state.attributes.get("friendly_name") or action_entity
        self.manager.async_send_event(
            screen_id,
            {
                "kind": "doorbell",
                "id": ring_id,
                "title": title,
                "stream_url": f"{DOORBELL_URL}/{ring_id}.mjpeg" if camera else "",
                "timeout": timeout,
                "action": action,
            },
        )
        return ring_id

    async def async_action(
        self, screen_id: str, ring_id: str, context: Context | None = None
    ) -> None:
        """Run the screen's doorbell action (open the gate, ...). Raises on failure."""
        ring = self.rings.get(ring_id)
        if ring is None or ring.screen_id != screen_id:
            raise HomeAssistantError("This doorbell ring is over")
        record = self.manager.screens.get(screen_id)
        entity_id = record.settings.get("doorbell_action") if record else ""
        if not entity_id:
            raise HomeAssistantError("No doorbell action is set for this screen")
        domain, service = ACTION_SERVICES[entity_id.split(".", 1)[0]]
        await self.hass.services.async_call(
            domain, service, {"entity_id": entity_id}, blocking=True, context=context
        )

    def ring_for_stream(self, ring_id: str) -> Ring | None:
        """The ring whose stream may be opened now."""
        self._purge()
        return self.rings.get(ring_id)

    def _purge(self) -> None:
        now = time.monotonic()
        for ring_id in [r for r, ring in self.rings.items() if ring.expires + GRACE < now]:
            del self.rings[ring_id]


def ffmpeg_args(binary: str, source: str) -> list[str]:
    """ffmpeg command line: `source` to small MJPEG on stdout."""
    width, height = STREAM_MAX_SIZE
    args = [binary, "-hide_banner", "-loglevel", "error", "-nostdin"]
    if source.startswith("rtsp"):
        args += ["-rtsp_transport", "tcp"]
    args += [
        "-i", source,
        "-an",
        "-vf",
        f"fps={STREAM_FPS},"
        f"scale={width}:{height}:force_original_aspect_ratio=decrease,"
        "scale=trunc(iw/2)*2:trunc(ih/2)*2",
        "-c:v", "mjpeg",
        "-pix_fmt", "yuvj420p",
        "-q:v", str(STREAM_QUALITY),
        "-f", "mpjpeg",
        "-boundary_tag", BOUNDARY,
        "pipe:1",
    ]
    return args


def _ffmpeg_binary(hass: HomeAssistant) -> str | None:
    if "ffmpeg" in hass.config.components:
        from homeassistant.components.ffmpeg import get_ffmpeg_manager

        return get_ffmpeg_manager(hass).binary
    return shutil.which("ffmpeg")


class DoorbellStreamView(HomeAssistantView):
    """MJPEG of a ring's camera, for the screen it rang on."""

    url = DOORBELL_URL + "/{ring_id}.mjpeg"
    name = "api:esp32_screen:doorbell"
    requires_auth = True

    async def get(self, request: web.Request, ring_id: str) -> web.StreamResponse:
        """Stream until the ring times out or the screen hangs up."""
        hass = request.app[KEY_HASS]
        manager: ScreenManager | None = hass.data.get(DOMAIN)
        ring = manager.doorbells.ring_for_stream(ring_id) if manager else None
        if ring is None or not ring.camera:
            return web.Response(status=404)
        remaining = max(5.0, ring.expires - time.monotonic())

        from homeassistant.components.camera import async_get_stream_source

        try:
            source = await async_get_stream_source(hass, ring.camera)
        except HomeAssistantError as err:
            _LOGGER.debug("No stream source for %s: %s", ring.camera, err)
            source = None
        binary = _ffmpeg_binary(hass) if source else None
        response = web.StreamResponse(
            headers={
                hdrs.CONTENT_TYPE: f"multipart/x-mixed-replace;boundary={BOUNDARY}",
                hdrs.CACHE_CONTROL: "no-store",
            }
        )
        await response.prepare(request)
        try:
            if source and binary:
                await _stream_ffmpeg(response, binary, source, remaining)
            else:
                await _stream_snapshots(hass, response, ring.camera, remaining)
        except (ConnectionResetError, asyncio.CancelledError):
            # The screen hung up.
            pass
        return response


async def _stream_ffmpeg(
    response: web.StreamResponse, binary: str, source: str, seconds: float
) -> None:
    proc = await asyncio.create_subprocess_exec(
        *ffmpeg_args(binary, source),
        stdin=asyncio.subprocess.DEVNULL,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
    )
    assert proc.stdout is not None
    loop = asyncio.get_running_loop()
    deadline = loop.time() + seconds
    try:
        while (left := deadline - loop.time()) > 0:
            try:
                chunk = await asyncio.wait_for(proc.stdout.read(32768), timeout=min(left, 15))
            except TimeoutError:
                break
            if not chunk:
                break
            await response.write(chunk)
    finally:
        if proc.returncode is None:
            proc.kill()
        _, err = await proc.communicate()
        if proc.returncode not in (0, -9) and err:
            _LOGGER.warning("ffmpeg for the doorbell stream: %s", err.decode(errors="replace").strip()[-300:])


async def _stream_snapshots(
    hass: HomeAssistant, response: web.StreamResponse, camera: str, seconds: float
) -> None:
    from .notifications import fetch_image

    loop = asyncio.get_running_loop()
    deadline = loop.time() + seconds
    while loop.time() < deadline:
        started = loop.time()
        try:
            data = await fetch_image(hass, camera)
        except HomeAssistantError as err:
            _LOGGER.debug("Doorbell snapshot: %s", err)
            await asyncio.sleep(1)
            continue
        await response.write(
            f"--{BOUNDARY}\r\nContent-Type: image/jpeg\r\nContent-Length: {len(data)}\r\n\r\n".encode()
            + data
            + b"\r\n"
        )
        await asyncio.sleep(max(0.0, SNAPSHOT_INTERVAL - (loop.time() - started)))

