"""Screen records, persistence, live subscriptions and actions."""

from __future__ import annotations

import asyncio
from collections.abc import Callable
import copy
from dataclasses import dataclass, field
from datetime import datetime, timedelta
import logging
from typing import Any

import voluptuous as vol

from homeassistant.components import websocket_api
from homeassistant.config_entries import ConfigEntry
from homeassistant.core import (
    CALLBACK_TYPE,
    Context,
    Event,
    EventStateChangedData,
    HomeAssistant,
    State,
    callback,
)
from homeassistant.exceptions import HomeAssistantError
from homeassistant.helpers import device_registry as dr
from homeassistant.helpers.event import (
    async_track_state_change_event,
    async_track_time_interval,
)
from homeassistant.helpers.storage import Store
from homeassistant.util import dt as dt_util

from .const import (
    DOMAIN,
    ERR_INVALID_ACTION,
    ERR_SERVICE_FAILED,
    KIND_ACTIONS,
    KIND_CLIMATE,
    ALARM_MODES,
    KIND_ALARM,
    KIND_BLIND,
    KIND_LOCK,
    KIND_MEDIA,
    KIND_SCENE,
    KIND_TOGGLE,
    MANUFACTURER,
    FORECAST_MIN_INTERVAL,
    FORECAST_REFRESH,
    SENSOR_FIELDS,
    default_settings,
    STORAGE_KEY,
    STORAGE_VERSION,
)
from .firmware import FirmwareImage, FirmwareStore
from .doorbell import Doorbells
from .forecast import async_forecast
from .notifications import NotificationImages
from .layout import empty_layout, iter_tiles, upgrade_layout
from .view_model import info_payload, resolve_label, tile_view_model

_LOGGER = logging.getLogger(__name__)

SAVE_DELAY = 5.0
# Automation tiles show "Ran 5 min ago"; refresh that text this often.
RELATIVE_TIME_REFRESH = timedelta(seconds=60)


class ActionError(Exception):
    """An action could not be performed. `code` is the websocket error code."""

    def __init__(self, code: str, message: str) -> None:
        """Initialize."""
        super().__init__(message)
        self.code = code


@dataclass
class ScreenRecord:
    """A stored screen."""

    screen_id: str
    name: str
    name_by_user: bool = False
    model: str | None = None
    sw_version: str | None = None
    last_seen: str | None = None
    layout: dict[str, Any] = field(default_factory=empty_layout)
    settings: dict[str, Any] = field(default_factory=default_settings)

    def as_storage(self) -> dict[str, Any]:
        """Serialize for the store."""
        return {
            "name": self.name,
            "name_by_user": self.name_by_user,
            "model": self.model,
            "sw_version": self.sw_version,
            "last_seen": self.last_seen,
            "layout": self.layout,
            "settings": self.settings,
        }

    @classmethod
    def from_storage(cls, screen_id: str, data: dict[str, Any]) -> ScreenRecord:
        """Deserialize from the store."""
        return cls(
            screen_id=screen_id,
            name=data.get("name") or default_name(screen_id),
            name_by_user=bool(data.get("name_by_user", False)),
            model=data.get("model"),
            sw_version=data.get("sw_version"),
            last_seen=data.get("last_seen"),
            layout=upgrade_layout(data.get("layout") or empty_layout()),
            settings={**default_settings(), **(data.get("settings") or {})},
        )


def default_name(screen_id: str) -> str:
    """Name used before the screen registers itself."""
    return f"ESP32 Screen {screen_id[-4:]}"


class ScreenSubscription:
    """One open `esp32_screen/subscribe` of a screen."""

    def __init__(
        self,
        manager: ScreenManager,
        connection: websocket_api.ActiveConnection,
        msg_id: int,
        screen_id: str,
    ) -> None:
        """Initialize."""
        self.manager = manager
        self.hass = manager.hass
        self.connection = connection
        self.msg_id = msg_id
        self.screen_id = screen_id
        self._sent: dict[str, dict[str, Any]] = {}
        self._labels: dict[str, str] = {}
        self._tiles: list[dict[str, Any]] = []
        # Entity id -> tiles that depend on it (main entity or a climate sensor).
        self._tiles_by_entity: dict[str, list[dict[str, Any]]] = {}
        self._unsub_track: CALLBACK_TYPE | None = None
        self._unsub_timer: CALLBACK_TYPE | None = None
        self._unsub_info: CALLBACK_TYPE | None = None
        self._info: dict[str, Any] | None = None
        self._unsub_forecast_timer: CALLBACK_TYPE | None = None
        self._forecast: dict[str, Any] | None = None
        self._forecast_task: asyncio.Task[None] | None = None
        self._forecast_at: datetime | None = None

    @callback
    def start(self) -> None:
        """Send the layout and all states, and start tracking entities."""
        self.send_full()

    @callback
    def stop(self) -> None:
        """Stop tracking."""
        if self._unsub_track is not None:
            self._unsub_track()
            self._unsub_track = None
        if self._unsub_timer is not None:
            self._unsub_timer()
            self._unsub_timer = None
        self._stop_info()

    @callback
    def _stop_info(self) -> None:
        if self._unsub_info is not None:
            self._unsub_info()
            self._unsub_info = None
        if self._unsub_forecast_timer is not None:
            self._unsub_forecast_timer()
            self._unsub_forecast_timer = None
        if self._forecast_task is not None:
            self._forecast_task.cancel()
            self._forecast_task = None

    @callback
    def send_full(self) -> None:
        """(Re)send `layout` and every tile's state, re-tracking entities."""
        self.stop()
        record = self.manager.screens.get(self.screen_id)
        layout = copy.deepcopy(record.layout) if record else empty_layout()
        self._tiles_by_entity = {}
        self._tiles = []
        self._labels = {}
        for tile in iter_tiles(layout):
            label = resolve_label(self.hass, tile)
            self._labels[tile["id"]] = label
            raw = dict(tile)
            self._tiles.append(raw)
            for entity_id in {tile["entity_id"], *(tile.get(f) for f in SENSOR_FIELDS)}:
                if entity_id:
                    self._tiles_by_entity.setdefault(entity_id, []).append(raw)
            tile["label"] = label

        self.connection.send_event(self.msg_id, {"kind": "layout", "layout": layout})

        self._sent = {}
        tiles: dict[str, dict[str, Any]] = {}
        for tile in self._tiles:
            vm = self.manager.view_model(self.screen_id, tile)
            self._sent[tile["id"]] = vm
            tiles[tile["id"]] = vm
        self.connection.send_event(self.msg_id, {"kind": "states", "tiles": tiles})

        if self._tiles_by_entity:
            self._unsub_track = async_track_state_change_event(
                self.hass, list(self._tiles_by_entity), self._state_changed
            )
        if any(t["entity_id"].startswith("automation.") for t in self._tiles):
            self._unsub_timer = async_track_time_interval(
                self.hass, self._refresh_relative, RELATIVE_TIME_REFRESH
            )
        self.send_settings()

    @callback
    def send_settings(self) -> None:
        """Send `settings`, `info` and `forecast`, and (re)track the info entities."""
        self._stop_info()
        record = self.manager.screens.get(self.screen_id)
        settings = record.settings if record else default_settings()
        self.connection.send_event(
            self.msg_id,
            {
                "kind": "settings",
                "settings": {
                    "language": settings["language"],
                    "clock_24h": settings["clock_24h"],
                },
            },
        )
        self._info = None
        self._push_info()
        entities = [
            e
            for e in (settings["weather_entity"], settings["indoor_temperature_entity"])
            if e
        ]
        if entities:
            self._unsub_info = async_track_state_change_event(
                self.hass, entities, self._info_changed
            )
        self._forecast = None
        self._refresh_forecast(force=True)
        self._unsub_forecast_timer = async_track_time_interval(
            self.hass,
            lambda _now: self._refresh_forecast(force=True),
            timedelta(seconds=FORECAST_REFRESH),
            name="esp32_screen forecast",
        )

    @callback
    def _info_changed(self, event: Event[EventStateChangedData]) -> None:
        self._push_info()
        if event.data["entity_id"].startswith("weather."):
            self._refresh_forecast()

    @callback
    def _refresh_forecast(self, force: bool = False) -> None:
        """Fetch the forecast and send it when it changed. Without `force`,
        at most every FORECAST_MIN_INTERVAL (weather entities update often)."""
        if self._forecast_task is not None and not self._forecast_task.done():
            return
        now = dt_util.utcnow()
        if (
            not force
            and self._forecast_at is not None
            and (now - self._forecast_at).total_seconds() < FORECAST_MIN_INTERVAL
        ):
            return
        self._forecast_at = now
        self._forecast_task = self.hass.async_create_background_task(
            self._push_forecast(), f"esp32_screen forecast {self.screen_id}"
        )

    async def _push_forecast(self) -> None:
        record = self.manager.screens.get(self.screen_id)
        settings = record.settings if record else default_settings()
        forecast = await async_forecast(self.hass, settings["weather_entity"])
        if forecast != self._forecast:
            self._forecast = forecast
            self.connection.send_event(self.msg_id, {"kind": "forecast", **forecast})

    @callback
    def _push_info(self) -> None:
        record = self.manager.screens.get(self.screen_id)
        settings = record.settings if record else default_settings()
        info = info_payload(self.hass, settings)
        if info != self._info:
            self._info = info
            self.connection.send_event(self.msg_id, {"kind": "info", **info})

    @callback
    def _refresh_relative(self, _now: datetime) -> None:
        self._push_changed(
            [t for t in self._tiles if t["entity_id"].startswith("automation.")]
        )

    @callback
    def _state_changed(self, event: Event[EventStateChangedData]) -> None:
        entity_id = event.data["entity_id"]
        tiles = self._tiles_by_entity.get(entity_id, [])
        # A tile with an empty label shows the friendly name, which is only
        # known once the entity exists. Resend the layout when it changes.
        if any(
            not tile.get("label")
            and tile["entity_id"] == entity_id
            and resolve_label(self.hass, tile) != self._labels.get(tile["id"])
            for tile in tiles
        ):
            self.send_full()
            return
        self._push_changed(tiles)

    @callback
    def _push_changed(self, tiles: list[dict[str, Any]]) -> None:
        changed: dict[str, dict[str, Any]] = {}
        for tile in tiles:
            vm = self.manager.view_model(self.screen_id, tile)
            if self._sent.get(tile["id"]) != vm:
                self._sent[tile["id"]] = vm
                changed[tile["id"]] = vm
        if changed:
            self.connection.send_event(
                self.msg_id, {"kind": "states", "tiles": changed}
            )


@dataclass
class OtaJob:
    """An install in progress on one screen."""

    image: FirmwareImage
    future: asyncio.Future[None]
    state: str = "pending"
    progress: int = 0


class ScreenManager:
    """Owns screen records and live subscriptions."""

    def __init__(self, hass: HomeAssistant, entry: ConfigEntry) -> None:
        """Initialize."""
        self.hass = hass
        self.entry = entry
        self.store: Store[dict[str, Any]] = Store(hass, STORAGE_VERSION, STORAGE_KEY)
        self.screens: dict[str, ScreenRecord] = {}
        self._subscriptions: dict[str, set[ScreenSubscription]] = {}
        self._listeners: set[Callable[[], None]] = set()
        # Last non-off hvac mode per (screen_id, tile_id), memory only.
        self._last_hvac_mode: dict[tuple[str, str], str] = {}
        self.firmware = FirmwareStore(hass)
        self.ota_jobs: dict[str, OtaJob] = {}
        # Last esp32_screen/state report per online screen (memory only).
        self.device_states: dict[str, dict[str, Any]] = {}
        self.notification_images = NotificationImages()
        self.doorbells = Doorbells(self)
        # Called with a screen_id when its install progress or record changes.
        self._screen_listeners: set[Callable[[str], None]] = set()

    # ---- persistence -------------------------------------------------------

    async def async_load(self) -> None:
        """Load records from storage and make sure every screen has a device."""
        data = await self.store.async_load() or {}
        for screen_id, raw in (data.get("screens") or {}).items():
            self.screens[screen_id] = ScreenRecord.from_storage(screen_id, raw)
        for record in self.screens.values():
            self._update_device(record)
        self.doorbells.async_track()

    @callback
    def _data_to_save(self) -> dict[str, Any]:
        return {
            "screens": {sid: rec.as_storage() for sid, rec in self.screens.items()}
        }

    async def async_save_now(self) -> None:
        await self.store.async_save(self._data_to_save())

    @callback
    def _schedule_save(self) -> None:
        self.store.async_delay_save(self._data_to_save, SAVE_DELAY)

    @callback
    def async_shutdown(self) -> None:
        """Close all subscriptions (config entry unload)."""
        self.doorbells.async_stop()
        for subs in self._subscriptions.values():
            for sub in subs:
                sub.stop()
        self._subscriptions.clear()
        self._listeners.clear()
        self._screen_listeners.clear()
        for job in self.ota_jobs.values():
            if not job.future.done():
                job.future.set_exception(HomeAssistantError("Integration unloaded"))
        self.ota_jobs.clear()

    # ---- device registry ---------------------------------------------------

    @callback
    def _update_device(self, record: ScreenRecord) -> None:
        dr.async_get(self.hass).async_get_or_create(
            config_entry_id=self.entry.entry_id,
            identifiers={(DOMAIN, record.screen_id)},
            manufacturer=MANUFACTURER,
            model=record.model,
            sw_version=record.sw_version,
            name=record.name,
            serial_number=record.screen_id,
        )

    # ---- listeners (editor) --------------------------------------------------

    @callback
    def async_add_listener(self, listener: Callable[[], None]) -> CALLBACK_TYPE:
        """Call `listener` whenever a record or an online state changes."""
        self._listeners.add(listener)

        @callback
        def remove() -> None:
            self._listeners.discard(listener)

        return remove

    @callback
    def _notify(self) -> None:
        for listener in list(self._listeners):
            listener()
        for screen_id in list(self.screens):
            self._notify_screen(screen_id)

    @callback
    def async_add_screen_listener(self, listener: Callable[[str], None]) -> CALLBACK_TYPE:
        """Call `listener(screen_id)` on record, online or install changes."""
        self._screen_listeners.add(listener)

        @callback
        def remove() -> None:
            self._screen_listeners.discard(listener)

        return remove

    @callback
    def _notify_screen(self, screen_id: str) -> None:
        for listener in list(self._screen_listeners):
            listener(screen_id)

    # ---- screen controls -------------------------------------------------------

    @callback
    def async_send_event(self, screen_id: str, event: dict[str, Any]) -> int:
        """Send an event on every open subscription of a screen. Returns how many."""
        subs = self._subscriptions.get(screen_id, set())
        for sub in subs:
            sub.connection.send_event(sub.msg_id, event)
        return len(subs)

    @callback
    def async_send_command(self, screen_id: str, command: str, value: Any = None) -> None:
        """Send a `command` event. Raises when the screen is offline."""
        if not self.async_send_event(
            screen_id, {"kind": "command", "command": command, "value": value}
        ):
            raise HomeAssistantError("Screen is offline")

    @callback
    def async_device_state(self, screen_id: str, state: dict[str, Any]) -> None:
        """State report from a screen (esp32_screen/state)."""
        if self.device_states.get(screen_id) != state:
            self.device_states[screen_id] = state
            self._notify_screen(screen_id)

    # ---- firmware updates ----------------------------------------------------

    @callback
    def async_start_ota(self, screen_id: str, image: FirmwareImage) -> OtaJob:
        """Send the `ota` event to the screen. Raises when it is offline."""
        subs = self._subscriptions.get(screen_id)
        if not subs:
            raise HomeAssistantError("Screen is offline")
        if (old := self.ota_jobs.get(screen_id)) is not None and not old.future.done():
            raise HomeAssistantError("An update is already running on this screen")
        job = OtaJob(image, self.hass.loop.create_future())
        self.ota_jobs[screen_id] = job
        event = {
            "kind": "ota",
            "version": image.version,
            "size": image.size,
            "sha256": image.sha256,
            "url": image.url,
        }
        _LOGGER.info("Sending firmware %s to screen %s", image.version, screen_id)
        for sub in subs:
            sub.connection.send_event(sub.msg_id, event)
        self._notify_screen(screen_id)
        return job

    @callback
    def async_finish_ota(self, screen_id: str, job: OtaJob) -> None:
        """Forget a finished (or abandoned) install."""
        if self.ota_jobs.get(screen_id) is job:
            del self.ota_jobs[screen_id]
            if not job.future.done():
                job.future.cancel()
        self._notify_screen(screen_id)

    @callback
    def async_ota_status(
        self, screen_id: str, state: str, progress: int, error: str | None
    ) -> None:
        """Progress report from a screen."""
        if (record := self.screens.get(screen_id)) is not None:
            self._touch(record)
        if state == "failed":
            _LOGGER.warning(
                "Firmware update on screen %s failed: %s", screen_id, error or "no reason given"
            )
        else:
            _LOGGER.debug("Firmware update on %s: %s %s%%", screen_id, state, progress)
        job = self.ota_jobs.get(screen_id)
        if job is None:
            return
        job.state = state
        job.progress = max(0, min(100, progress))
        if not job.future.done():
            if state == "restarting":
                job.future.set_result(None)
            elif state == "failed":
                job.future.set_exception(
                    HomeAssistantError(error or "The screen reported a failed update")
                )
        self._notify_screen(screen_id)

    def is_online(self, screen_id: str) -> bool:
        """Return True while the screen has an open subscription."""
        return bool(self._subscriptions.get(screen_id))

    @callback
    def screens_payload(self) -> list[dict[str, Any]]:
        """Return the screen list for the editor."""
        return [
            {
                "screen_id": rec.screen_id,
                "name": rec.name,
                "model": rec.model,
                "sw_version": rec.sw_version,
                "online": self.is_online(rec.screen_id),
                "last_seen": rec.last_seen,
                "layout": rec.layout,
                "settings": rec.settings,
            }
            for rec in sorted(self.screens.values(), key=lambda r: r.name.lower())
        ]

    @callback
    def _touch(self, record: ScreenRecord) -> None:
        record.last_seen = dt_util.utcnow().isoformat()
        self._schedule_save()

    # ---- screen commands -----------------------------------------------------

    @callback
    def async_register(
        self,
        screen_id: str,
        name: str | None,
        model: str | None,
        sw_version: str | None,
    ) -> ScreenRecord:
        """Create or update a screen record from the screen itself."""
        record = self.screens.get(screen_id)
        if record is None:
            record = ScreenRecord(screen_id=screen_id, name=name or default_name(screen_id))
            self.screens[screen_id] = record
        elif name and not record.name_by_user:
            record.name = name
        if model is not None:
            record.model = model
        if sw_version is not None:
            record.sw_version = sw_version
        self._touch(record)
        self._update_device(record)
        self._notify()
        return record

    @callback
    def async_subscribe(
        self, connection: websocket_api.ActiveConnection, msg_id: int, screen_id: str
    ) -> CALLBACK_TYPE:
        """Start a screen subscription. Returns the unsubscribe callback."""
        sub = ScreenSubscription(self, connection, msg_id, screen_id)
        self._subscriptions.setdefault(screen_id, set()).add(sub)
        if (record := self.screens.get(screen_id)) is not None:
            self._touch(record)
        sub.start()
        self._notify()

        @callback
        def unsubscribe() -> None:
            sub.stop()
            subs = self._subscriptions.get(screen_id)
            if subs is not None:
                subs.discard(sub)
                if not subs:
                    del self._subscriptions[screen_id]
                    # Offline: forget the reported state; it is resent on subscribe.
                    self.device_states.pop(screen_id, None)
            if (record := self.screens.get(screen_id)) is not None:
                self._touch(record)
            self._notify()

        return unsubscribe

    @callback
    def view_model(self, screen_id: str, tile: dict[str, Any]) -> dict[str, Any]:
        """Return a tile's view model and remember climate modes."""
        state = self.hass.states.get(tile["entity_id"])
        vm = tile_view_model(self.hass, tile, state)
        if tile["kind"] == KIND_CLIMATE and vm["mode"] not in ("", "off"):
            self._last_hvac_mode[(screen_id, tile["id"])] = vm["mode"]
        return vm

    # ---- editor commands -----------------------------------------------------

    async def async_save_layout(self, screen_id: str, layout: dict[str, Any]) -> None:
        """Store a validated layout, creating the record if needed."""
        record = self.screens.get(screen_id)
        if record is None:
            record = ScreenRecord(screen_id=screen_id, name=default_name(screen_id))
            self.screens[screen_id] = record
            self._update_device(record)
        record.layout = layout
        await self.async_save_now()
        for sub in list(self._subscriptions.get(screen_id, ())):
            sub.send_full()
        self._notify()

    async def async_update_settings(self, screen_id: str, changes: dict[str, Any]) -> bool:
        """Change a screen's settings. Returns False when unknown."""
        if (record := self.screens.get(screen_id)) is None:
            return False
        record.settings = {**record.settings, **changes}
        await self.async_save_now()
        if any(key.startswith("doorbell_") for key in changes):
            self.doorbells.async_track()
        for sub in list(self._subscriptions.get(screen_id, ())):
            sub.send_settings()
        self._notify()
        return True

    async def async_rename(self, screen_id: str, name: str) -> bool:
        """Rename a screen. Returns False when unknown."""
        if (record := self.screens.get(screen_id)) is None:
            return False
        record.name = name
        record.name_by_user = True
        self._update_device(record)
        await self.async_save_now()
        self._notify()
        return True

    async def async_remove(self, screen_id: str, *, remove_device: bool = True) -> bool:
        """Forget a screen. Returns False when unknown."""
        if self.screens.pop(screen_id, None) is None:
            return False
        for key in [k for k in self._last_hvac_mode if k[0] == screen_id]:
            del self._last_hvac_mode[key]
        if remove_device:
            registry = dr.async_get(self.hass)
            identifier = (DOMAIN, screen_id)
            if hasattr(registry, "async_get_device_by_identifier"):
                # HA 2026.x; async_get_device(identifiers=...) is deprecated there.
                device = registry.async_get_device_by_identifier(
                    identifier, self.entry.entry_id
                )
            else:
                device = registry.async_get_device(identifiers={identifier})
            if device:
                registry.async_remove_device(device.id)
        await self.async_save_now()
        # A connected screen keeps its subscription; it now shows nothing.
        for sub in list(self._subscriptions.get(screen_id, ())):
            sub.send_full()
        self._notify()
        return True

    # ---- actions ---------------------------------------------------------------

    def find_tile(self, screen_id: str, tile_id: str) -> dict[str, Any] | None:
        """Return a tile of a screen's layout."""
        if (record := self.screens.get(screen_id)) is None:
            return None
        for tile in iter_tiles(record.layout):
            if tile["id"] == tile_id:
                return tile
        return None

    async def async_action(
        self,
        screen_id: str,
        tile: dict[str, Any],
        action: str,
        value: Any,
        context: Context,
    ) -> None:
        """Run a tile action. Raises ActionError."""
        kind: str = tile["kind"]
        if action not in KIND_ACTIONS[kind]:
            raise ActionError(
                ERR_INVALID_ACTION, f"{kind} tiles do not support '{action}'"
            )
        entity_id: str = tile["entity_id"]
        state = self.hass.states.get(entity_id)
        if state is None:
            raise ActionError(ERR_SERVICE_FAILED, f"{entity_id} does not exist")
        if (record := self.screens.get(screen_id)) is not None:
            self._touch(record)

        service, data = self._resolve_service(screen_id, tile, state, action, value)
        data = {"entity_id": entity_id, **data}
        _LOGGER.debug("%s/%s: %s.%s %s", screen_id, tile["id"], service[0], service[1], data)
        try:
            await self.hass.services.async_call(
                service[0], service[1], data, blocking=True, context=context
            )
        except (HomeAssistantError, vol.Invalid, ValueError) as err:
            raise ActionError(
                ERR_SERVICE_FAILED, f"{service[0]}.{service[1]} failed: {err}"
            ) from err

    def _resolve_service(
        self,
        screen_id: str,
        tile: dict[str, Any],
        state: State,
        action: str,
        value: Any,
    ) -> tuple[tuple[str, str], dict[str, Any]]:
        domain: str = state.domain
        current: str = state.state

        def number(low: float | None = None, high: float | None = None) -> float:
            if isinstance(value, bool) or not isinstance(value, (int, float)):
                raise ActionError(ERR_INVALID_ACTION, f"'{action}' needs a number value")
            if (low is not None and value < low) or (high is not None and value > high):
                raise ActionError(
                    ERR_INVALID_ACTION, f"value must be between {low} and {high}"
                )
            return float(value)

        kind = tile["kind"]
        if kind == KIND_TOGGLE:
            service = {"toggle": "toggle", "on": "turn_on", "off": "turn_off"}[action]
            return (domain, service), {}

        def code() -> dict[str, Any]:
            if value is None or value == "":
                return {}
            if not isinstance(value, str):
                raise ActionError(ERR_INVALID_ACTION, "the code must be a string")
            return {"code": value}

        features = int(state.attributes.get("supported_features") or 0)

        if kind == KIND_SCENE:  # activate / toggle
            return (domain, "press" if domain in ("button", "input_button") else "turn_on"), {}

        if kind == KIND_LOCK:
            if action == "toggle":
                action = "unlock" if current == "locked" else "lock"
            return ("lock", action), code()

        if kind == KIND_ALARM:
            if action == "disarm":
                return ("alarm_control_panel", "alarm_disarm"), code()
            bit = dict(ALARM_MODES)[action]
            if not features & bit:
                raise ActionError(ERR_INVALID_ACTION, f"{state.entity_id} does not support {action}")
            return ("alarm_control_panel", f"alarm_{action}"), code()

        if kind == KIND_MEDIA:
            if action == "toggle":
                return ("media_player", "turn_on" if current == "off" else "turn_off"), {}
            if action == "volume":
                return ("media_player", "volume_set"), {"volume_level": round(number(0, 100)) / 100}
            service = {
                "play_pause": "media_play_pause",
                "next": "media_next_track",
                "previous": "media_previous_track",
            }[action]
            return ("media_player", service), {}

        if kind == KIND_BLIND and action == "position":
            if not features & 4:  # CoverEntityFeature.SET_POSITION
                raise ActionError(ERR_INVALID_ACTION, f"{state.entity_id} cannot be set to a position")
            return ("cover", "set_cover_position"), {"position": round(number(0, 100))}

        if domain == "light":
            if action == "toggle":
                return ("light", "toggle"), {}
            if action == "hue":
                return ("light", "turn_on"), {"hs_color": [round(number(0, 360)) % 360, 100]}
            if action == "kelvin":
                low = state.attributes.get("min_color_temp_kelvin") or 1000
                high = state.attributes.get("max_color_temp_kelvin") or 40000
                return ("light", "turn_on"), {"color_temp_kelvin": round(number(low, high))}
            pct = round(number(0, 100))
            if pct == 0:
                return ("light", "turn_off"), {}
            return ("light", "turn_on"), {"brightness_pct": pct}

        if domain == "switch":
            if action == "toggle":
                return ("switch", "toggle"), {}
            if action == "open":
                return ("switch", "turn_on"), {}
            if action == "close":
                return ("switch", "turn_off"), {}
            if action == "brightness":
                return ("switch", "turn_on" if number(0, 100) > 0 else "turn_off"), {}
            raise ActionError(ERR_INVALID_ACTION, f"a switch cannot '{action}'")

        if domain == "button":
            if action in ("toggle", "open", "close"):
                return ("button", "press"), {}
            raise ActionError(ERR_INVALID_ACTION, f"a button cannot '{action}'")

        if domain in ("cover", "valve"):
            open_s, close_s, stop_s = (
                ("open_cover", "close_cover", "stop_cover")
                if domain == "cover"
                else ("open_valve", "close_valve", "stop_valve")
            )
            features = int(state.attributes.get("supported_features") or 0)
            # STOP is 8 for both CoverEntityFeature and ValveEntityFeature.
            can_stop = bool(features & 8)
            if action == "toggle":
                if current in ("opening", "closing") and can_stop:
                    return (domain, stop_s), {}
                if current in ("open", "opening"):
                    return (domain, close_s), {}
                return (domain, open_s), {}
            if action == "open":
                return (domain, open_s), {}
            if action == "close":
                return (domain, close_s), {}
            if action == "stop":
                if not can_stop:
                    raise ActionError(ERR_INVALID_ACTION, f"{state.entity_id} cannot stop")
                return (domain, stop_s), {}

        if domain == "climate":
            key = (screen_id, tile["id"])
            modes = [str(m) for m in state.attributes.get("hvac_modes") or []]
            if action == "toggle":
                if current not in ("off", "unknown", "unavailable"):
                    self._last_hvac_mode[key] = current
                    return ("climate", "set_hvac_mode"), {"hvac_mode": "off"}
                mode = self._last_hvac_mode.get(key)
                if mode is None or mode not in modes:
                    mode = next((m for m in modes if m != "off"), None)
                if mode is None:
                    raise ActionError(
                        ERR_INVALID_ACTION, f"{state.entity_id} has no mode to turn on"
                    )
                return ("climate", "set_hvac_mode"), {"hvac_mode": mode}
            if action == "set_temperature":
                return ("climate", "set_temperature"), {"temperature": number()}
            if action == "set_mode":
                if not isinstance(value, str) or value not in modes:
                    raise ActionError(
                        ERR_INVALID_ACTION,
                        f"mode must be one of: {', '.join(modes)}",
                    )
                return ("climate", "set_hvac_mode"), {"hvac_mode": value}

        raise ActionError(
            ERR_INVALID_ACTION, f"'{action}' is not supported for {state.entity_id}"
        )
