"""Websocket commands (docs/protocol.md, sections 3 and 4)."""

from __future__ import annotations

from functools import lru_cache
from importlib import resources
from pathlib import Path
from typing import Any

import voluptuous as vol

from homeassistant.components import websocket_api
from homeassistant.core import HomeAssistant, callback

from .const import (
    DOMAIN,
    ERR_INVALID_LAYOUT,
    ERR_NOT_LOADED,
    MAX_NAME_LEN,
    OTA_STATES,
    SCREEN_ID_PATTERN,
    SCREENSAVER_MODES,
    LANGUAGES,
)
from .layout import ENTITY_ID_RE, LayoutError, validate_layout
from .manager import ActionError, ScreenManager

SCREEN_ID = vol.All(str, vol.Match(SCREEN_ID_PATTERN, msg="screen_id must be 12 upper-case hex digits"))
OPTIONAL_TEXT = vol.Any(None, vol.All(str, vol.Length(max=MAX_NAME_LEN)))
NAME = vol.All(str, vol.Strip, vol.Length(min=1, max=MAX_NAME_LEN))


@callback
def async_register_commands(hass: HomeAssistant) -> None:
    """Register all websocket commands."""
    for handler in (
        ws_register,
        ws_subscribe,
        ws_action,
        ws_ota_status,
        ws_state,
        ws_screens,
        ws_save_layout,
        ws_update_screen,
        ws_remove_screen,
        ws_subscribe_screens,
    ):
        websocket_api.async_register_command(hass, handler)


def _manager(
    hass: HomeAssistant, connection: websocket_api.ActiveConnection, msg: dict[str, Any]
) -> ScreenManager | None:
    manager: ScreenManager | None = hass.data.get(DOMAIN)
    if manager is None:
        connection.send_error(
            msg["id"], ERR_NOT_LOADED, "The ESP32 Screen integration is not set up"
        )
    return manager


@lru_cache(maxsize=4)
def _posix_tz(zone: str) -> str:
    """POSIX TZ rule for an IANA zone ("Europe/Lisbon" -> "WET0WEST,M3.5.0/1,M10.5.0").

    The screens' C library only understands POSIX rules. Version 2+ TZif files
    end with that rule as a footer line. Blocking I/O: run in the executor.
    """
    data = b""
    try:
        data = resources.files("tzdata").joinpath("zoneinfo", *zone.split("/")).read_bytes()
    except (ModuleNotFoundError, FileNotFoundError, OSError, ValueError):
        try:
            data = (Path("/usr/share/zoneinfo") / zone).read_bytes()
        except OSError:
            return ""
    if not data.startswith(b"TZif") or data[4:5] < b"2":
        return ""
    footer = data.rstrip(b"\n").rsplit(b"\n", 1)[-1]
    try:
        return footer.decode("ascii")
    except UnicodeDecodeError:
        return ""


# ---- commands used by the screen (any authenticated user) -----------------------


@websocket_api.websocket_command(
    {
        vol.Required("type"): "esp32_screen/register",
        vol.Required("screen_id"): SCREEN_ID,
        vol.Optional("name"): OPTIONAL_TEXT,
        vol.Optional("model"): OPTIONAL_TEXT,
        vol.Optional("sw_version"): OPTIONAL_TEXT,
    }
)
@websocket_api.async_response
async def ws_register(
    hass: HomeAssistant, connection: websocket_api.ActiveConnection, msg: dict[str, Any]
) -> None:
    """Create or update a screen record."""
    if (manager := _manager(hass, connection, msg)) is None:
        return
    name = (msg.get("name") or "").strip() or None
    record = manager.async_register(
        msg["screen_id"], name, msg.get("model"), msg.get("sw_version")
    )
    time_zone = hass.config.time_zone
    posix_tz = await hass.async_add_executor_job(_posix_tz, time_zone)
    connection.send_result(
        msg["id"],
        {
            "screen_id": record.screen_id,
            "name": record.name,
            "time_zone": time_zone,
            "posix_tz": posix_tz,
        },
    )


@websocket_api.websocket_command(
    {
        vol.Required("type"): "esp32_screen/subscribe",
        vol.Required("screen_id"): SCREEN_ID,
    }
)
@callback
def ws_subscribe(
    hass: HomeAssistant, connection: websocket_api.ActiveConnection, msg: dict[str, Any]
) -> None:
    """Stream layout and tile states to a screen."""
    if (manager := _manager(hass, connection, msg)) is None:
        return
    connection.send_result(msg["id"])
    connection.subscriptions[msg["id"]] = manager.async_subscribe(
        connection, msg["id"], msg["screen_id"]
    )


@websocket_api.websocket_command(
    {
        vol.Required("type"): "esp32_screen/action",
        vol.Required("screen_id"): SCREEN_ID,
        vol.Required("tile_id"): str,
        vol.Required("action"): str,
        vol.Optional("value"): vol.Any(int, float, str, None),
    }
)
@websocket_api.async_response
async def ws_action(
    hass: HomeAssistant, connection: websocket_api.ActiveConnection, msg: dict[str, Any]
) -> None:
    """Perform a tile action."""
    if (manager := _manager(hass, connection, msg)) is None:
        return
    screen_id: str = msg["screen_id"]
    if screen_id not in manager.screens:
        connection.send_error(
            msg["id"], websocket_api.ERR_NOT_FOUND, f"Unknown screen {screen_id}"
        )
        return
    tile = manager.find_tile(screen_id, msg["tile_id"])
    if tile is None:
        connection.send_error(
            msg["id"], websocket_api.ERR_NOT_FOUND, f"Unknown tile {msg['tile_id']}"
        )
        return
    try:
        await manager.async_action(
            screen_id, tile, msg["action"], msg.get("value"), connection.context(msg)
        )
    except ActionError as err:
        connection.send_error(msg["id"], err.code, str(err))
        return
    connection.send_result(msg["id"])


@websocket_api.websocket_command(
    {
        vol.Required("type"): "esp32_screen/ota_status",
        vol.Required("screen_id"): SCREEN_ID,
        vol.Required("state"): vol.In(OTA_STATES),
        vol.Optional("progress", default=0): vol.All(
            vol.Coerce(int), vol.Range(min=0, max=100)
        ),
        vol.Optional("error"): vol.Any(None, vol.All(str, vol.Length(max=255))),
    }
)
@callback
def ws_ota_status(
    hass: HomeAssistant, connection: websocket_api.ActiveConnection, msg: dict[str, Any]
) -> None:
    """Firmware update progress from a screen."""
    if (manager := _manager(hass, connection, msg)) is None:
        return
    if msg["screen_id"] not in manager.screens:
        connection.send_error(
            msg["id"], websocket_api.ERR_NOT_FOUND, f"Unknown screen {msg['screen_id']}"
        )
        return
    manager.async_ota_status(
        msg["screen_id"], msg["state"], msg["progress"], msg.get("error")
    )
    connection.send_result(msg["id"])


@websocket_api.websocket_command(
    {
        vol.Required("type"): "esp32_screen/state",
        vol.Required("screen_id"): SCREEN_ID,
        vol.Required("brightness"): vol.All(int, vol.Range(min=0, max=100)),
        vol.Required("screensaver_mode"): vol.In(SCREENSAVER_MODES),
        vol.Required("screensaver_delay"): vol.All(int, vol.Range(min=0, max=86400)),
        vol.Required("screensaver_active"): bool,
    }
)
@callback
def ws_state(
    hass: HomeAssistant, connection: websocket_api.ActiveConnection, msg: dict[str, Any]
) -> None:
    """Brightness and screensaver state reported by a screen."""
    if (manager := _manager(hass, connection, msg)) is None:
        return
    if msg["screen_id"] not in manager.screens:
        connection.send_error(
            msg["id"], websocket_api.ERR_NOT_FOUND, f"Unknown screen {msg['screen_id']}"
        )
        return
    manager.async_device_state(
        msg["screen_id"],
        {
            key: msg[key]
            for key in ("brightness", "screensaver_mode", "screensaver_delay", "screensaver_active")
        },
    )
    connection.send_result(msg["id"])


# ---- commands used by the editor panel (admin only) ------------------------------


@websocket_api.websocket_command({vol.Required("type"): "esp32_screen/screens"})
@websocket_api.require_admin
@callback
def ws_screens(
    hass: HomeAssistant, connection: websocket_api.ActiveConnection, msg: dict[str, Any]
) -> None:
    """List screens."""
    if (manager := _manager(hass, connection, msg)) is None:
        return
    connection.send_result(msg["id"], {"screens": manager.screens_payload()})


@websocket_api.websocket_command(
    {
        vol.Required("type"): "esp32_screen/layout/save",
        vol.Required("screen_id"): SCREEN_ID,
        vol.Required("layout"): dict,
    }
)
@websocket_api.require_admin
@websocket_api.async_response
async def ws_save_layout(
    hass: HomeAssistant, connection: websocket_api.ActiveConnection, msg: dict[str, Any]
) -> None:
    """Validate and store a layout."""
    if (manager := _manager(hass, connection, msg)) is None:
        return
    try:
        layout = validate_layout(msg["layout"])
    except LayoutError as err:
        connection.send_error(msg["id"], ERR_INVALID_LAYOUT, str(err))
        return
    await manager.async_save_layout(msg["screen_id"], layout)
    connection.send_result(msg["id"])


def _entity_of(domain: str) -> Any:
    def validate(value: Any) -> str:
        if value is None or value == "":
            return ""
        if not isinstance(value, str) or not ENTITY_ID_RE.match(value) or not value.startswith(f"{domain}."):
            raise vol.Invalid(f"must be \"\" or a {domain}.* entity id")
        return value

    return validate


SETTINGS_SCHEMA = vol.Schema(
    {
        vol.Optional("weather_entity"): _entity_of("weather"),
        vol.Optional("indoor_temperature_entity"): _entity_of("sensor"),
        vol.Optional("language"): vol.In(LANGUAGES),
        vol.Optional("clock_24h"): bool,
    }
)


@websocket_api.websocket_command(
    {
        vol.Required("type"): "esp32_screen/screen/update",
        vol.Required("screen_id"): SCREEN_ID,
        vol.Optional("name"): NAME,
        vol.Optional("settings"): SETTINGS_SCHEMA,
    }
)
@websocket_api.require_admin
@websocket_api.async_response
async def ws_update_screen(
    hass: HomeAssistant, connection: websocket_api.ActiveConnection, msg: dict[str, Any]
) -> None:
    """Rename a screen and/or change its settings."""
    if (manager := _manager(hass, connection, msg)) is None:
        return
    screen_id = msg["screen_id"]
    if screen_id not in manager.screens:
        connection.send_error(
            msg["id"], websocket_api.ERR_NOT_FOUND, f"Unknown screen {screen_id}"
        )
        return
    if "name" in msg:
        await manager.async_rename(screen_id, msg["name"])
    if msg.get("settings"):
        await manager.async_update_settings(screen_id, msg["settings"])
    connection.send_result(msg["id"])


@websocket_api.websocket_command(
    {
        vol.Required("type"): "esp32_screen/screen/remove",
        vol.Required("screen_id"): SCREEN_ID,
    }
)
@websocket_api.require_admin
@websocket_api.async_response
async def ws_remove_screen(
    hass: HomeAssistant, connection: websocket_api.ActiveConnection, msg: dict[str, Any]
) -> None:
    """Forget a screen and its device."""
    if (manager := _manager(hass, connection, msg)) is None:
        return
    if not await manager.async_remove(msg["screen_id"]):
        connection.send_error(
            msg["id"], websocket_api.ERR_NOT_FOUND, f"Unknown screen {msg['screen_id']}"
        )
        return
    connection.send_result(msg["id"])


@websocket_api.websocket_command({vol.Required("type"): "esp32_screen/subscribe_screens"})
@websocket_api.require_admin
@callback
def ws_subscribe_screens(
    hass: HomeAssistant, connection: websocket_api.ActiveConnection, msg: dict[str, Any]
) -> None:
    """Send the screen list now and whenever it changes."""
    if (manager := _manager(hass, connection, msg)) is None:
        return

    @callback
    def send() -> None:
        connection.send_event(msg["id"], {"screens": manager.screens_payload()})

    connection.subscriptions[msg["id"]] = manager.async_add_listener(send)
    connection.send_result(msg["id"])
    send()
