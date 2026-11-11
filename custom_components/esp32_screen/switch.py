"""Weather forecast screensaver switch (docs/protocol.md, section 6)."""

from __future__ import annotations

from typing import Any

from homeassistant.components.switch import SwitchEntity
from homeassistant.config_entries import ConfigEntry
from homeassistant.const import EntityCategory
from homeassistant.core import HomeAssistant
from homeassistant.helpers.entity_platform import AddConfigEntryEntitiesCallback

from .entity import ScreenControlEntity, async_setup_screen_entities
from .manager import ScreenManager


async def async_setup_entry(
    hass: HomeAssistant,
    entry: ConfigEntry,
    async_add_entities: AddConfigEntryEntitiesCallback,
) -> None:
    """One forecast switch per screen."""
    async_setup_screen_entities(hass, entry, async_add_entities, lambda m, sid: [WeatherScreensaver(m, sid)])


class WeatherScreensaver(ScreenControlEntity, SwitchEntity):
    """Show the 7-day forecast as a screensaver, alone or taking turns with the clock."""

    _attr_entity_category = EntityCategory.CONFIG

    def __init__(self, manager: ScreenManager, screen_id: str) -> None:
        """Initialize."""
        super().__init__(manager, screen_id, "screensaver_weather")

    @property
    def available(self) -> bool:
        """Only screens whose firmware has the forecast report this."""
        return super().available and "screensaver_weather" in (self.device_state or {})

    @property
    def is_on(self) -> bool | None:
        """Reported setting."""
        state = self.device_state
        return state.get("screensaver_weather") if state else None

    async def async_turn_on(self, **kwargs: Any) -> None:
        """Turn the forecast on."""
        await self._send("screensaver_weather", True)

    async def async_turn_off(self, **kwargs: Any) -> None:
        """Turn the forecast off."""
        await self._send("screensaver_weather", False)
