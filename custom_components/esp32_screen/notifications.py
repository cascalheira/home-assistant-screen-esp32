"""Notifications on the screens (docs/protocol.md, section 8)."""

from __future__ import annotations

from dataclasses import dataclass
import io
import logging
import secrets
import time

from aiohttp import hdrs, web
import voluptuous as vol

from homeassistant.components.http import HomeAssistantView
from homeassistant.core import (
    HomeAssistant,
    ServiceCall,
    ServiceResponse,
    SupportsResponse,
    callback,
)
from homeassistant.exceptions import HomeAssistantError, ServiceValidationError
from homeassistant.helpers import config_validation as cv, device_registry as dr
from homeassistant.helpers.http import KEY_HASS

from .const import DOMAIN

_LOGGER = logging.getLogger(__name__)

SERVICE_NOTIFY = "notify"
SERVICE_DISMISS = "dismiss"
NOTIFICATION_URL = "/api/esp32_screen/notification"
IMAGE_MAX_SIZE = (480, 300)
IMAGE_QUALITY = 80
IMAGE_TTL = 600  # seconds
MAX_TITLE = 100
MAX_MESSAGE = 500

NOTIFY_SCHEMA = vol.Schema(
    {
        vol.Optional("device_id"): vol.All(cv.ensure_list, [cv.string]),
        vol.Optional("title", default=""): vol.All(cv.string, vol.Length(max=MAX_TITLE)),
        vol.Required("message"): vol.All(cv.string, vol.Length(max=MAX_MESSAGE)),
        vol.Optional("image_entity"): vol.Any(
            None, vol.All(cv.entity_id, cv.entity_domain(["camera", "image"]))
        ),
        vol.Optional("timeout", default=30): vol.All(
            vol.Coerce(int), vol.Range(min=0, max=86400)
        ),
    }
)

DISMISS_SCHEMA = vol.Schema(
    {
        vol.Optional("device_id"): vol.All(cv.ensure_list, [cv.string]),
        vol.Optional("id", default=""): cv.string,
    }
)


@dataclass
class StoredImage:
    """A converted notification image."""

    data: bytes
    expires: float


class NotificationImages:
    """In-memory JPEGs for notifications, dropped after IMAGE_TTL."""

    def __init__(self) -> None:
        """Initialize."""
        self._images: dict[str, StoredImage] = {}

    def add(self, image_id: str, data: bytes) -> None:
        """Store an image."""
        self.purge()
        self._images[image_id] = StoredImage(data, time.monotonic() + IMAGE_TTL)

    def get(self, image_id: str) -> bytes | None:
        """Return an image that has not expired."""
        self.purge()
        stored = self._images.get(image_id)
        return stored.data if stored else None

    def purge(self) -> None:
        """Drop expired images."""
        now = time.monotonic()
        for key in [k for k, v in self._images.items() if v.expires <= now]:
            del self._images[key]


def to_jpeg(raw: bytes) -> bytes:
    """Baseline JPEG that fits 480x300, aspect ratio kept (runs in the executor)."""
    from PIL import Image  # Pillow ships with Home Assistant

    with Image.open(io.BytesIO(raw)) as img:
        img.load()
        if img.mode in ("RGBA", "LA", "P"):
            rgba = img.convert("RGBA")
            background = Image.new("RGB", rgba.size, (0, 0, 0))
            background.paste(rgba, mask=rgba.split()[-1])
            out = background
        else:
            out = img.convert("RGB")
        out.thumbnail(IMAGE_MAX_SIZE, Image.Resampling.LANCZOS)
        buf = io.BytesIO()
        out.save(buf, "JPEG", quality=IMAGE_QUALITY, progressive=False, optimize=True)
        return buf.getvalue()


async def _fetch_image(hass: HomeAssistant, entity_id: str) -> bytes:
    domain = entity_id.split(".", 1)[0]
    try:
        if domain == "camera":
            from homeassistant.components.camera import async_get_image

            image = await async_get_image(hass, entity_id)
        else:
            from homeassistant.components.image import (
                async_get_image as async_get_image_entity,
            )

            image = await async_get_image_entity(hass, entity_id)
    except HomeAssistantError as err:
        raise HomeAssistantError(f"Cannot get an image from {entity_id}: {err}") from err
    try:
        return await hass.async_add_executor_job(to_jpeg, image.content)
    except (OSError, ValueError) as err:
        raise HomeAssistantError(f"Cannot convert the image of {entity_id}: {err}") from err


def _target_screens(hass: HomeAssistant, device_ids: list[str] | None) -> list[str]:
    """Screen ids for the given devices, or all screens."""
    manager = hass.data.get(DOMAIN)
    if manager is None:
        raise HomeAssistantError("The ESP32 Screen integration is not set up")
    if not device_ids:
        return list(manager.screens)
    registry = dr.async_get(hass)
    screens: list[str] = []
    for device_id in device_ids:
        device = registry.async_get(device_id)
        ids = [i[1] for i in device.identifiers if i[0] == DOMAIN] if device else []
        if not ids:
            raise ServiceValidationError(f"Device {device_id} is not an ESP32 screen")
        screens.extend(ids)
    return screens


@callback
def async_register_services(hass: HomeAssistant) -> None:
    """Register esp32_screen.notify and esp32_screen.dismiss."""

    async def notify(call: ServiceCall) -> ServiceResponse:
        manager = hass.data.get(DOMAIN)
        targets = _target_screens(hass, call.data.get("device_id"))
        online = [sid for sid in targets if manager.is_online(sid)]
        if call.data.get("device_id") and not online:
            raise HomeAssistantError("None of the target screens is online")
        notification_id = secrets.token_hex(8)
        image_url = ""
        if entity_id := call.data.get("image_entity"):
            manager.notification_images.add(notification_id, await _fetch_image(hass, entity_id))
            image_url = f"{NOTIFICATION_URL}/{notification_id}.jpg"
        event = {
            "kind": "notify",
            "id": notification_id,
            "title": call.data["title"],
            "message": call.data["message"],
            "image_url": image_url,
            "timeout": call.data["timeout"],
        }
        for screen_id in online:
            manager.async_send_event(screen_id, event)
        _LOGGER.debug("Notification %s sent to %s", notification_id, online)
        return {"id": notification_id, "screens": online}

    async def dismiss(call: ServiceCall) -> None:
        manager = hass.data.get(DOMAIN)
        for screen_id in _target_screens(hass, call.data.get("device_id")):
            manager.async_send_event(screen_id, {"kind": "dismiss", "id": call.data["id"]})

    hass.services.async_register(
        DOMAIN, SERVICE_NOTIFY, notify, schema=NOTIFY_SCHEMA,
        supports_response=SupportsResponse.OPTIONAL,
    )
    hass.services.async_register(DOMAIN, SERVICE_DISMISS, dismiss, schema=DISMISS_SCHEMA)


class NotificationImageView(HomeAssistantView):
    """Serves notification images (any authenticated user)."""

    url = NOTIFICATION_URL + "/{image_id}.jpg"
    name = "api:esp32_screen:notification"
    requires_auth = True

    async def get(self, request: web.Request, image_id: str) -> web.Response:
        """Return the JPEG."""
        hass: HomeAssistant = request.app[KEY_HASS]
        manager = hass.data.get(DOMAIN)
        data = manager.notification_images.get(image_id) if manager else None
        if data is None:
            return web.Response(status=404, text="Unknown or expired notification image")
        return web.Response(
            body=data,
            headers={hdrs.CONTENT_TYPE: "image/jpeg", hdrs.CACHE_CONTROL: "no-store"},
        )

