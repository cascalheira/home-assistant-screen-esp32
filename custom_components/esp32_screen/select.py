"""Screensaver mode select (docs/protocol.md, section 6)."""

from __future__ import annotations

from homeassistant.components.select import SelectEntity
from homeassistant.config_entries import ConfigEntry
from homeassistant.const import EntityCategory
from homeassistant.core import HomeAssistant
from homeassistant.helpers.entity_platform import AddConfigEntryEntitiesCallback

from .const import SCREENSAVER_MODES
from .entity import ScreenControlEntity, async_setup_screen_entities
from .manager import ScreenManager


async def async_setup_entry(
    hass: HomeAssistant,
    entry: ConfigEntry,
    async_add_entities: AddConfigEntryEntitiesCallback,
) -> None:
    """One screensaver select per screen."""
    async_setup_screen_entities(hass, entry, async_add_entities, lambda m, sid: [ScreensaverMode(m, sid)])


class ScreensaverMode(ScreenControlEntity, SelectEntity):
    """off / clock / flip."""

    _attr_options = list(SCREENSAVER_MODES)
    _attr_entity_category = EntityCategory.CONFIG

    def __init__(self, manager: ScreenManager, screen_id: str) -> None:
        """Initialize."""
        super().__init__(manager, screen_id, "screensaver")

    @property
    def current_option(self) -> str | None:
        """Reported mode."""
        state = self.device_state
        return state.get("screensaver_mode") if state else None

    async def async_select_option(self, option: str) -> None:
        """Send the new mode."""
        await self._send("screensaver_mode", option)
