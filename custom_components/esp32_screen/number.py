"""Screensaver delay in minutes (docs/protocol.md, section 6)."""

from __future__ import annotations

from homeassistant.components.number import NumberEntity, NumberMode
from homeassistant.config_entries import ConfigEntry
from homeassistant.const import EntityCategory, UnitOfTime
from homeassistant.core import HomeAssistant
from homeassistant.helpers.entity_platform import AddConfigEntryEntitiesCallback

from .entity import ScreenControlEntity, async_setup_screen_entities
from .manager import ScreenManager


async def async_setup_entry(
    hass: HomeAssistant,
    entry: ConfigEntry,
    async_add_entities: AddConfigEntryEntitiesCallback,
) -> None:
    """One delay number per screen."""
    async_setup_screen_entities(hass, entry, async_add_entities, lambda m, sid: [ScreensaverDelay(m, sid)])


class ScreensaverDelay(ScreenControlEntity, NumberEntity):
    """Idle minutes before the screensaver starts (the screen works in seconds)."""

    _attr_native_min_value = 1
    _attr_native_max_value = 60
    _attr_native_step = 1
    _attr_native_unit_of_measurement = UnitOfTime.MINUTES
    _attr_mode = NumberMode.BOX
    _attr_entity_category = EntityCategory.CONFIG

    def __init__(self, manager: ScreenManager, screen_id: str) -> None:
        """Initialize."""
        super().__init__(manager, screen_id, "screensaver_delay")

    @property
    def native_value(self) -> float | None:
        """Reported delay in minutes."""
        state = self.device_state
        if state is None:
            return None
        return round(int(state.get("screensaver_delay", 0)) / 60, 1)

    async def async_set_native_value(self, value: float) -> None:
        """Send the delay in seconds."""
        await self._send("screensaver_delay", round(value * 60))
