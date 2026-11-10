"""Screen backlight as a light (docs/protocol.md, section 6)."""

from __future__ import annotations

from typing import Any

from homeassistant.components.light import ATTR_BRIGHTNESS, ColorMode, LightEntity
from homeassistant.config_entries import ConfigEntry
from homeassistant.core import HomeAssistant
from homeassistant.helpers.entity_platform import AddConfigEntryEntitiesCallback

from .entity import ScreenControlEntity, async_setup_screen_entities
from .manager import ScreenManager


async def async_setup_entry(
    hass: HomeAssistant,
    entry: ConfigEntry,
    async_add_entities: AddConfigEntryEntitiesCallback,
) -> None:
    """One backlight per screen."""
    async_setup_screen_entities(hass, entry, async_add_entities, lambda m, sid: [ScreenBacklight(m, sid)])


class ScreenBacklight(ScreenControlEntity, LightEntity):
    """Backlight brightness 0-100 on the screen, 0 = off."""

    _attr_color_mode = ColorMode.BRIGHTNESS
    _attr_supported_color_modes = {ColorMode.BRIGHTNESS}

    def __init__(self, manager: ScreenManager, screen_id: str) -> None:
        """Initialize."""
        super().__init__(manager, screen_id, "backlight")
        self._last_on = 100

    @property
    def _percent(self) -> int:
        state = self.device_state or {}
        return int(state.get("brightness", 0))

    @property
    def is_on(self) -> bool | None:
        """On when the brightness is above 0."""
        if self.device_state is None:
            return None
        return self._percent > 0

    @property
    def brightness(self) -> int | None:
        """HA brightness 1-255."""
        if not self.is_on:
            return None
        return max(1, round(self._percent * 255 / 100))

    async def async_turn_on(self, **kwargs: Any) -> None:
        """Set a brightness, or restore the last one."""
        if ATTR_BRIGHTNESS in kwargs:
            percent = max(1, round(int(kwargs[ATTR_BRIGHTNESS]) * 100 / 255))
        elif self._percent > 0:
            return
        else:
            percent = self._last_on
        await self._send("brightness", percent)

    async def async_turn_off(self, **kwargs: Any) -> None:
        """Backlight off (brightness 0)."""
        if self._percent > 0:
            self._last_on = self._percent
        await self._send("brightness", 0)
