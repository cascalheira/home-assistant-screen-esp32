"""Wake and show-clock buttons (docs/protocol.md, section 6)."""

from __future__ import annotations

from homeassistant.components.button import ButtonEntity
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
    """Two buttons per screen."""
    async_setup_screen_entities(
        hass,
        entry,
        async_add_entities,
        lambda m, sid: [ScreenButton(m, sid, "wake", "wake"), ScreenButton(m, sid, "show_clock", "sleep")],
    )


class ScreenButton(ScreenControlEntity, ButtonEntity):
    """Sends one command without a value."""

    def __init__(self, manager: ScreenManager, screen_id: str, key: str, command: str) -> None:
        """Initialize."""
        super().__init__(manager, screen_id, key)
        self._command = command

    @property
    def available(self) -> bool:
        """Only needs the screen to be online."""
        return self.manager.is_online(self.screen_id)

    async def async_press(self) -> None:
        """Send the command."""
        await self._send(self._command)
