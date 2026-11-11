"""Screensaver selects (docs/protocol.md, section 6)."""

from __future__ import annotations

from homeassistant.components.select import SelectEntity
from homeassistant.config_entries import ConfigEntry
from homeassistant.const import EntityCategory
from homeassistant.core import HomeAssistant
from homeassistant.helpers.entity_platform import AddConfigEntryEntitiesCallback

from .const import SCREENSAVER_MODES, SCREENSAVER_ROTATE
from .entity import ScreenControlEntity, async_setup_screen_entities
from .manager import ScreenManager


async def async_setup_entry(
    hass: HomeAssistant,
    entry: ConfigEntry,
    async_add_entities: AddConfigEntryEntitiesCallback,
) -> None:
    """Screensaver clock and rotation selects per screen."""
    async_setup_screen_entities(
        hass, entry, async_add_entities,
        lambda m, sid: [ScreensaverMode(m, sid), ScreensaverRotation(m, sid)],
    )


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


class ScreensaverRotation(ScreenControlEntity, SelectEntity):
    """Seconds between the clock and the forecast when both are on."""

    _attr_options = [str(s) for s in SCREENSAVER_ROTATE]
    _attr_entity_category = EntityCategory.CONFIG

    def __init__(self, manager: ScreenManager, screen_id: str) -> None:
        """Initialize."""
        super().__init__(manager, screen_id, "screensaver_rotate")

    @property
    def available(self) -> bool:
        """Only screens whose firmware has the forecast report this."""
        return super().available and "screensaver_rotate" in (self.device_state or {})

    @property
    def current_option(self) -> str | None:
        """Reported interval."""
        state = self.device_state
        value = state.get("screensaver_rotate") if state else None
        return str(value) if value is not None else None

    async def async_select_option(self, option: str) -> None:
        """Send the new interval."""
        await self._send("screensaver_rotate", int(option))
