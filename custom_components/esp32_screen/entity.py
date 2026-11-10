"""Shared bits for the per-screen entities."""

from __future__ import annotations

from collections.abc import Callable, Iterable
from typing import Any

from homeassistant.config_entries import ConfigEntry
from homeassistant.core import HomeAssistant, callback
from homeassistant.helpers.device_registry import DeviceInfo
from homeassistant.helpers.entity import Entity
from homeassistant.helpers.entity_platform import AddConfigEntryEntitiesCallback

from .const import DOMAIN
from .manager import ScreenManager


@callback
def async_setup_screen_entities(
    hass: HomeAssistant,
    entry: ConfigEntry,
    async_add_entities: AddConfigEntryEntitiesCallback,
    factory: Callable[[ScreenManager, str], Iterable[Entity]],
) -> None:
    """Add `factory(manager, screen_id)` entities for every screen, now and later."""
    manager: ScreenManager = hass.data[DOMAIN]
    known: set[str] = set()

    @callback
    def sync(_screen_id: str | None = None) -> None:
        # A removed screen loses its device, which removes its entities.
        known.intersection_update(manager.screens)
        new = [sid for sid in manager.screens if sid not in known]
        if new:
            known.update(new)
            async_add_entities(entity for sid in new for entity in factory(manager, sid))

    sync()
    entry.async_on_unload(manager.async_add_screen_listener(sync))


class ScreenEntity(Entity):
    """An entity on a screen's device, updated from the manager."""

    _attr_has_entity_name = True
    _attr_should_poll = False

    def __init__(self, manager: ScreenManager, screen_id: str, key: str) -> None:
        """Initialize."""
        self.manager = manager
        self.screen_id = screen_id
        self._attr_unique_id = f"{screen_id}_{key}"
        self._attr_translation_key = key
        self._attr_device_info = DeviceInfo(identifiers={(DOMAIN, screen_id)})

    async def async_added_to_hass(self) -> None:
        """Follow screen changes."""
        self.async_on_remove(self.manager.async_add_screen_listener(self._screen_changed))

    @callback
    def _screen_changed(self, screen_id: str) -> None:
        if screen_id == self.screen_id and self.hass is not None and self.entity_id:
            self.async_write_ha_state()


class ScreenControlEntity(ScreenEntity):
    """Mirrors the state the screen reports with esp32_screen/state."""

    @property
    def device_state(self) -> dict[str, Any] | None:
        """Last reported state, or None."""
        return self.manager.device_states.get(self.screen_id)

    @property
    def available(self) -> bool:
        """Available while the screen is online and has reported its state."""
        return self.manager.is_online(self.screen_id) and self.device_state is not None

    async def _send(self, command: str, value: Any = None) -> None:
        self.manager.async_send_command(self.screen_id, command, value)
