"""Firmware update entity, one per screen (docs/protocol.md, section 5)."""

from __future__ import annotations

import asyncio
from typing import Any

from homeassistant.components.update import (
    UpdateDeviceClass,
    UpdateEntity,
    UpdateEntityFeature,
)
from homeassistant.config_entries import ConfigEntry
from homeassistant.core import HomeAssistant, callback
from homeassistant.exceptions import HomeAssistantError
from homeassistant.helpers.device_registry import DeviceInfo
from homeassistant.helpers.entity_platform import AddConfigEntryEntitiesCallback

from .const import DOMAIN, OTA_TIMEOUT
from .entity import async_setup_screen_entities
from .manager import ScreenManager


async def async_setup_entry(
    hass: HomeAssistant,
    entry: ConfigEntry,
    async_add_entities: AddConfigEntryEntitiesCallback,
) -> None:
    """Create an update entity for every screen, now and when screens appear."""
    async_setup_screen_entities(
        hass, entry, async_add_entities, lambda m, sid: [ScreenFirmwareUpdate(m, sid)]
    )


class ScreenFirmwareUpdate(UpdateEntity):
    """Installs firmware from the firmware folder on one screen."""

    _attr_has_entity_name = True
    _attr_translation_key = "firmware"
    _attr_device_class = UpdateDeviceClass.FIRMWARE
    _attr_supported_features = UpdateEntityFeature.INSTALL | UpdateEntityFeature.PROGRESS
    _attr_title = "ESP32 Screen firmware"
    _attr_should_poll = False

    def __init__(self, manager: ScreenManager, screen_id: str) -> None:
        """Initialize."""
        self.manager = manager
        self.screen_id = screen_id
        self._attr_unique_id = f"{screen_id}_firmware"
        self._attr_device_info = DeviceInfo(identifiers={(DOMAIN, screen_id)})

    async def async_added_to_hass(self) -> None:
        """Follow screen and firmware changes."""
        self.async_on_remove(self.manager.async_add_screen_listener(self._screen_changed))
        self.async_on_remove(self.manager.firmware.async_add_listener(self.async_write_ha_state))

    @callback
    def _screen_changed(self, screen_id: str) -> None:
        if screen_id == self.screen_id and self.hass is not None and self.entity_id:
            self.async_write_ha_state()

    @property
    def available(self) -> bool:
        """Available while the screen record exists."""
        return self.screen_id in self.manager.screens

    @property
    def installed_version(self) -> str | None:
        """Version the screen reported in register."""
        record = self.manager.screens.get(self.screen_id)
        return record.sw_version if record else None

    @property
    def latest_version(self) -> str | None:
        """Highest valid version in the firmware folder."""
        latest = self.manager.firmware.latest
        return latest.version if latest else None

    @property
    def in_progress(self) -> bool:
        """True while an install runs on the screen."""
        return self.screen_id in self.manager.ota_jobs

    @property
    def update_percentage(self) -> int | None:
        """Download progress reported by the screen."""
        job = self.manager.ota_jobs.get(self.screen_id)
        return job.progress if job else None

    async def async_update(self) -> None:
        """`homeassistant.update_entity`: rescan the firmware folder."""
        await self.manager.firmware.async_scan()

    async def async_install(self, version: str | None, backup: bool, **kwargs: Any) -> None:
        """Send the latest image to the screen and wait until it restarts."""
        await self.manager.firmware.async_scan()
        image = self.manager.firmware.latest
        if image is None:
            raise HomeAssistantError("No firmware file in the firmware folder")
        if version is not None and version != image.version:
            raise HomeAssistantError(f"Only the latest firmware ({image.version}) can be installed")
        job = self.manager.async_start_ota(self.screen_id, image)
        try:
            async with asyncio.timeout(OTA_TIMEOUT):
                await job.future
        except TimeoutError as err:
            raise HomeAssistantError(
                f"The screen did not finish the update within {OTA_TIMEOUT // 60} minutes"
            ) from err
        finally:
            self.manager.async_finish_ota(self.screen_id, job)
