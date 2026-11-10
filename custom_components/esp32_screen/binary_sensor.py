"""Screensaver showing (docs/protocol.md, section 6)."""

from __future__ import annotations

from homeassistant.components.binary_sensor import BinarySensorEntity
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
    """One screensaver sensor per screen."""
    async_setup_screen_entities(hass, entry, async_add_entities, lambda m, sid: [ScreensaverActive(m, sid)])


class ScreensaverActive(ScreenControlEntity, BinarySensorEntity):
    """On while the screensaver is showing."""

    def __init__(self, manager: ScreenManager, screen_id: str) -> None:
        """Initialize."""
        super().__init__(manager, screen_id, "screensaver_active")

    @property
    def is_on(self) -> bool | None:
        """Reported screensaver state."""
        state = self.device_state
        return bool(state.get("screensaver_active")) if state else None
