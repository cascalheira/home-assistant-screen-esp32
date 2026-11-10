"""ESP32 Screen: wall screens driven by Home Assistant."""

from __future__ import annotations

from datetime import timedelta
import hashlib
from pathlib import Path

from homeassistant.components import frontend, panel_custom
from homeassistant.components.http import StaticPathConfig
from homeassistant.config_entries import ConfigEntry
from homeassistant.const import Platform
from homeassistant.core import HomeAssistant
from homeassistant.helpers import config_validation as cv
from homeassistant.helpers.device_registry import DeviceEntry
from homeassistant.helpers.event import async_track_time_interval
from homeassistant.helpers.typing import ConfigType

from .const import (
    DOMAIN,
    FIRMWARE_SCAN_INTERVAL,
    PANEL_COMPONENT,
    PANEL_FILE,
    PANEL_ICON,
    PANEL_TITLE,
    PANEL_URL_PATH,
    STATIC_URL,
)
from .firmware import FirmwareDownloadView
from .notifications import NotificationImageView, async_register_services
from .manager import ScreenManager
from .websocket import async_register_commands

CONFIG_SCHEMA = cv.config_entry_only_config_schema(DOMAIN)

FRONTEND_DIR = Path(__file__).parent / "frontend"
_STATIC_REGISTERED = f"{DOMAIN}_static_registered"
PLATFORMS = [
    Platform.BINARY_SENSOR,
    Platform.BUTTON,
    Platform.LIGHT,
    Platform.NUMBER,
    Platform.SELECT,
    Platform.UPDATE,
]


async def async_setup(hass: HomeAssistant, config: ConfigType) -> bool:
    """Register websocket commands once; they answer `not_loaded` without an entry."""
    async_register_commands(hass)
    async_register_services(hass)
    return True


def _file_hash(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()[:12]


async def async_setup_entry(hass: HomeAssistant, entry: ConfigEntry) -> bool:
    """Set up ESP32 Screen from a config entry."""
    manager = ScreenManager(hass, entry)
    await manager.async_load()
    hass.data[DOMAIN] = manager

    if not hass.data.get(_STATIC_REGISTERED):
        # Static paths cannot be unregistered, so only do this once per run.
        await hass.http.async_register_static_paths(
            [StaticPathConfig(STATIC_URL, str(FRONTEND_DIR), cache_headers=False)]
        )
        # Views cannot be unregistered either; it finds the manager in hass.data.
        hass.http.register_view(FirmwareDownloadView())
        hass.http.register_view(NotificationImageView())
        hass.data[_STATIC_REGISTERED] = True

    await manager.firmware.async_scan()

    async def _rescan(_now: object) -> None:
        await manager.firmware.async_scan()

    entry.async_on_unload(
        async_track_time_interval(
            hass, _rescan, timedelta(seconds=FIRMWARE_SCAN_INTERVAL),
            name="esp32_screen firmware scan",
        )
    )
    await hass.config_entries.async_forward_entry_setups(entry, PLATFORMS)

    version = await hass.async_add_executor_job(_file_hash, FRONTEND_DIR / PANEL_FILE)
    await panel_custom.async_register_panel(
        hass,
        frontend_url_path=PANEL_URL_PATH,
        webcomponent_name=PANEL_COMPONENT,
        sidebar_title=PANEL_TITLE,
        sidebar_icon=PANEL_ICON,
        module_url=f"{STATIC_URL}/{PANEL_FILE}?v={version}",
        embed_iframe=False,
        require_admin=True,
    )
    return True


async def async_unload_entry(hass: HomeAssistant, entry: ConfigEntry) -> bool:
    """Unload a config entry."""
    if not await hass.config_entries.async_unload_platforms(entry, PLATFORMS):
        return False
    frontend.async_remove_panel(hass, PANEL_URL_PATH, warn_if_unknown=False)
    manager: ScreenManager | None = hass.data.pop(DOMAIN, None)
    if manager is not None:
        await manager.async_save_now()
        manager.async_shutdown()
    return True


async def async_remove_config_entry_device(
    hass: HomeAssistant, entry: ConfigEntry, device: DeviceEntry
) -> bool:
    """Allow deleting a screen from the device page; forgets its layout too."""
    manager: ScreenManager | None = hass.data.get(DOMAIN)
    if manager is not None:
        for domain, screen_id in device.identifiers:
            if domain == DOMAIN:
                await manager.async_remove(screen_id, remove_device=False)
    return True
