"""Turn Home Assistant states into tile view models (docs/protocol.md, section 3)."""

from __future__ import annotations

from datetime import datetime
from typing import Any

from homeassistant.const import (
    ATTR_FRIENDLY_NAME,
    ATTR_UNIT_OF_MEASUREMENT,
    STATE_UNAVAILABLE,
    STATE_UNKNOWN,
    UnitOfTemperature,
)
from homeassistant.core import HomeAssistant, State
from homeassistant.exceptions import HomeAssistantError
from homeassistant.helpers import entity_registry as er
from homeassistant.util import dt as dt_util
from homeassistant.util.unit_conversion import TemperatureConverter

from .const import ALARM_MODES, KIND_SCENE, KIND_SENSOR, KIND_VALVE

HVAC_MODE_LABELS: dict[str, str] = {
    "off": "Off",
    "heat": "Heat",
    "cool": "Cool",
    "heat_cool": "Heat/Cool",
    "auto": "Auto",
    "dry": "Dry",
    "fan_only": "Fan",
}

HVAC_ACTION_LABELS: dict[str, str] = {
    "off": "Off",
    "idle": "Idle",
    "heating": "Heating",
    "cooling": "Cooling",
    "drying": "Drying",
    "fan": "Fan",
    "preheating": "Preheating",
    "defrosting": "Defrosting",
}

MOVING_STATES = ("opening", "closing")


COLOR_MODES = ("hs", "xy", "rgb", "rgbw", "rgbww")


def _light_color(
    vm: dict[str, Any], attrs: Any, modes: list[str], on: bool
) -> None:
    """Hue, colour temperature and the light's current colour."""
    if any(mode in COLOR_MODES for mode in modes):
        hs = attrs.get("hs_color")
        hue = _float(hs[0]) if isinstance(hs, (list, tuple)) and hs else None
        vm["hue"] = round(hue) % 360 if hue is not None else 0
    if "color_temp" in modes:
        low = _float(attrs.get("min_color_temp_kelvin"))
        high = _float(attrs.get("max_color_temp_kelvin"))
        if low and high and high > low:
            vm["kelvin_min"] = round(low)
            vm["kelvin_max"] = round(high)
            kelvin = _float(attrs.get("color_temp_kelvin"))
            vm["kelvin"] = round(min(max(kelvin, low), high)) if kelvin else 0
    rgb = attrs.get("rgb_color")
    if on and isinstance(rgb, (list, tuple)) and len(rgb) == 3:
        try:
            vm["rgb"] = "#" + "".join(f"{max(0, min(255, int(c))):02x}" for c in rgb)
        except (TypeError, ValueError):
            pass


def base_view_model() -> dict[str, Any]:
    """Return a view model with every key present and neutral values."""
    return {
        "available": False,
        "on": False,
        "status": "Unavailable",
        "detail": "",
        "moving": False,
        "brightness": -1,
        "current": None,
        "target": None,
        "min": None,
        "max": None,
        "step": None,
        "mode": "",
        "modes": [],
        "unit": "",
        "humidity": None,
        "position": -1,
        "volume": -1,
        "code_required": False,
        "hue": -1,
        "rgb": "",
        "kelvin": -1,
        "kelvin_min": -1,
        "kelvin_max": -1,
    }


def fmt_number(value: float) -> str:
    """Format a number with at most one decimal and no trailing .0."""
    rounded = round(float(value), 1)
    if rounded.is_integer():
        return str(int(rounded))
    return f"{rounded:.1f}"


def _float(value: Any) -> float | None:
    if isinstance(value, bool):
        return None
    if isinstance(value, (int, float)):
        return float(value)
    try:
        return float(value)
    except (TypeError, ValueError):
        return None


def resolve_label(hass: HomeAssistant, tile: dict[str, Any]) -> str:
    """Return the tile label, falling back to the entity's friendly name."""
    if tile.get("label"):
        return str(tile["label"])
    entity_id: str = tile["entity_id"]
    state = hass.states.get(entity_id)
    if state is not None and (name := state.attributes.get(ATTR_FRIENDLY_NAME)):
        return str(name)
    return entity_id.split(".", 1)[1].replace("_", " ").capitalize()


def tile_view_model(
    hass: HomeAssistant, tile: dict[str, Any], state: State | None
) -> dict[str, Any]:
    """Build the view model for one tile."""
    vm = base_view_model()
    if state is None or state.state == STATE_UNAVAILABLE:
        return vm

    vm["available"] = True
    domain = state.domain
    value = state.state
    attrs = state.attributes

    kind = tile.get("kind")
    if kind == KIND_SCENE:
        # Scenes and buttons have a timestamp (or unknown) as state.
        running = domain == "script" and value == "on"
        vm["on"] = running
        vm["status"] = "Running" if running else "Run"
        return vm

    if value == STATE_UNKNOWN and domain not in ("button", "climate"):
        vm["status"] = "Unknown"
        return vm

    if kind == KIND_SENSOR:
        return _sensor(hass, vm, state)

    if domain == "lock":
        vm["on"] = value != "locked"
        vm["status"] = LOCK_LABELS.get(value, value.replace("_", " ").capitalize())
        vm["moving"] = value in ("locking", "unlocking", "opening")
        vm["code_required"] = attrs.get("code_format") is not None
        return vm

    if domain == "alarm_control_panel":
        vm["on"] = value.startswith("armed_") or value == "triggered"
        vm["status"] = ALARM_LABELS.get(value, value.replace("_", " ").capitalize())
        features = int(attrs.get("supported_features") or 0)
        vm["modes"] = [mode for mode, bit in ALARM_MODES if features & bit]
        # A keypad is needed whenever the panel uses a code at all.
        vm["code_required"] = attrs.get("code_format") is not None
        return vm

    if domain == "media_player":
        return _media(vm, state)

    if domain == "light":
        on = value == "on"
        vm["on"] = on
        vm["status"] = "On" if on else "Off"
        modes = attrs.get("supported_color_modes") or []
        dimmable = any(mode not in ("onoff", "unknown") for mode in modes)
        if dimmable:
            raw = _float(attrs.get("brightness")) if on else 0.0
            pct = round((raw or 0.0) * 100 / 255) if on else 0
            if on and raw:
                pct = max(1, pct)
            vm["brightness"] = pct
            if on:
                vm["detail"] = f"{pct}%"
        _light_color(vm, attrs, modes, on)
        return vm

    if domain == "automation":
        on = value == "on"
        vm["on"] = on
        vm["status"] = "Enabled" if on else "Disabled"
        vm["detail"] = ran_ago(state.attributes.get("last_triggered"))
        return vm

    if domain == "input_boolean":
        on = value == "on"
        vm["on"] = on
        vm["status"] = "On" if on else "Off"
        return vm

    if domain == "switch":
        on = value == "on"
        vm["on"] = on
        if tile.get("kind") == KIND_VALVE:
            vm["status"] = "Open" if on else "Closed"
        else:
            vm["status"] = "On" if on else "Off"
        return vm

    if domain == "button":
        vm["status"] = "Press"
        return vm

    if domain in ("cover", "valve"):
        vm["on"] = value != "closed"
        vm["moving"] = value in MOVING_STATES
        vm["status"] = value.replace("_", " ").capitalize()
        position = _float(attrs.get("current_position"))
        if position is not None:
            vm["position"] = int(position)
            vm["detail"] = f"{int(position)}%"
        return vm

    if domain == "climate":
        return _climate(hass, vm, state, tile)

    vm["status"] = value.capitalize()
    return vm


LOCK_LABELS: dict[str, str] = {
    "locked": "Locked",
    "unlocked": "Unlocked",
    "locking": "Locking",
    "unlocking": "Unlocking",
    "jammed": "Jammed",
    "open": "Open",
    "opening": "Opening",
}

ALARM_LABELS: dict[str, str] = {
    "disarmed": "Disarmed",
    "armed_home": "Armed home",
    "armed_away": "Armed away",
    "armed_night": "Armed night",
    "armed_vacation": "Armed vacation",
    "armed_custom_bypass": "Armed custom",
    "arming": "Arming",
    "pending": "Pending",
    "triggered": "Triggered",
    "disarming": "Disarming",
}

MEDIA_LABELS: dict[str, str] = {
    "playing": "Playing",
    "paused": "Paused",
    "idle": "Idle",
    "off": "Off",
    "on": "On",
    "standby": "Standby",
    "buffering": "Buffering",
}

BINARY_WORDS: dict[str, tuple[str, str]] = {
    "door": ("Open", "Closed"),
    "garage_door": ("Open", "Closed"),
    "window": ("Open", "Closed"),
    "opening": ("Open", "Closed"),
    "motion": ("Detected", "Clear"),
    "occupancy": ("Detected", "Clear"),
    "presence": ("Detected", "Clear"),
    "moisture": ("Wet", "Dry"),
}

MEDIA_VOLUME_SET = 4  # MediaPlayerEntityFeature.VOLUME_SET


def _media(vm: dict[str, Any], state: State) -> dict[str, Any]:
    attrs = state.attributes
    vm["on"] = state.state == "playing"
    vm["status"] = MEDIA_LABELS.get(state.state, state.state.capitalize())
    artist = attrs.get("media_artist") or attrs.get("media_album_artist")
    title = attrs.get("media_title")
    if artist and title:
        vm["detail"] = f"{artist} – {title}"
    elif title:
        vm["detail"] = str(title)
    elif attrs.get("source"):
        vm["detail"] = str(attrs["source"])
    level = _float(attrs.get("volume_level"))
    if int(attrs.get("supported_features") or 0) & MEDIA_VOLUME_SET and level is not None:
        vm["volume"] = round(level * 100)
    return vm


def _display_precision(hass: HomeAssistant, entity_id: str) -> int | None:
    """User's display precision for a sensor, else the integration's suggestion."""
    entry = er.async_get(hass).async_get(entity_id)
    if entry is None:
        return None
    options = entry.options.get("sensor") or {}
    for key in ("display_precision", "suggested_display_precision"):
        if isinstance(options.get(key), int):
            return int(options[key])
    return None


def _sensor(hass: HomeAssistant, vm: dict[str, Any], state: State) -> dict[str, Any]:
    attrs = state.attributes
    if state.domain == "binary_sensor":
        on = state.state == "on"
        words = BINARY_WORDS.get(attrs.get("device_class") or "", ("On", "Off"))
        vm["on"] = on
        vm["status"] = words[0] if on else words[1]
        return vm
    value = state.state
    if attrs.get("device_class") == "timestamp" and (when := dt_util.parse_datetime(value)):
        local = dt_util.as_local(when)
        today = dt_util.now().date() == local.date()
        vm["status"] = local.strftime("%H:%M" if today else "%d %b %H:%M")
        return vm
    number = _float(value)
    if number is not None:
        precision = _display_precision(hass, state.entity_id)
        if precision is not None:
            value = f"{number:.{precision}f}"
    unit = attrs.get(ATTR_UNIT_OF_MEASUREMENT)
    vm["status"] = f"{value} {unit}" if unit else value
    return vm


def ran_ago(value: Any, now: datetime | None = None) -> str:
    """Short relative text for an automation's last_triggered attribute."""
    if isinstance(value, str):
        value = dt_util.parse_datetime(value)
    if not isinstance(value, datetime):
        return "Never ran"
    secs = ((now or dt_util.utcnow()) - value).total_seconds()
    if secs < 60:
        return "Ran just now"
    if secs < 3600:
        return f"Ran {int(secs // 60)} min ago"
    if secs < 86400:
        return f"Ran {int(secs // 3600)} h ago"
    return f"Ran {int(secs // 86400)} d ago"


def _sensor_value(hass: HomeAssistant, entity_id: str) -> tuple[float, str | None] | None:
    """Numeric state and unit of a sensor, or None when unusable."""
    if not entity_id or (state := hass.states.get(entity_id)) is None:
        return None
    if state.state in (STATE_UNAVAILABLE, STATE_UNKNOWN):
        return None
    value = _float(state.state)
    if value is None:
        return None
    return value, state.attributes.get(ATTR_UNIT_OF_MEASUREMENT)


def _sensor_temperature(hass: HomeAssistant, entity_id: str) -> float | None:
    """Sensor temperature converted to HA's temperature unit."""
    if (reading := _sensor_value(hass, entity_id)) is None:
        return None
    value, unit = reading
    target = hass.config.units.temperature_unit
    if unit and unit != target and unit in TemperatureConverter.VALID_UNITS:
        try:
            value = TemperatureConverter.convert(value, unit, target)
        except HomeAssistantError:
            return None
    return round(value, 1)


def _climate(
    hass: HomeAssistant, vm: dict[str, Any], state: State, tile: dict[str, Any]
) -> dict[str, Any]:
    attrs = state.attributes
    mode = state.state if state.state != STATE_UNKNOWN else ""
    unit = hass.config.units.temperature_unit
    vm["mode"] = mode
    vm["modes"] = [str(m) for m in attrs.get("hvac_modes") or []]
    vm["on"] = mode not in ("", "off")
    vm["unit"] = str(unit)

    action = attrs.get("hvac_action")
    if action and mode != "off":
        vm["status"] = HVAC_ACTION_LABELS.get(action, str(action).capitalize())
    elif mode:
        vm["status"] = HVAC_MODE_LABELS.get(mode, mode.capitalize())
    else:
        vm["status"] = "Unknown"

    current = _sensor_temperature(hass, tile.get("temperature_entity") or "")
    if current is None:
        current = _float(attrs.get("current_temperature"))
    humidity_reading = _sensor_value(hass, tile.get("humidity_entity") or "")
    humidity = (
        humidity_reading[0]
        if humidity_reading is not None
        else _float(attrs.get("current_humidity"))
    )
    vm["humidity"] = round(humidity) if humidity is not None else None
    target = _float(attrs.get("temperature"))
    vm["current"] = current
    vm["target"] = target
    vm["min"] = _float(attrs.get("min_temp"))
    vm["max"] = _float(attrs.get("max_temp"))
    step = _float(attrs.get("target_temp_step"))
    if step is None:
        step = 1.0 if unit == UnitOfTemperature.FAHRENHEIT else 0.5
    vm["step"] = step

    low = _float(attrs.get("target_temp_low"))
    high = _float(attrs.get("target_temp_high"))
    parts: list[str] = []
    if current is not None:
        parts.append(f"{fmt_number(current)} {unit}")
    if target is not None:
        parts.append(f"{fmt_number(target)} {unit}")
    elif low is not None and high is not None:
        parts.append(f"{fmt_number(low)}-{fmt_number(high)} {unit}")
    detail = " → ".join(parts)
    if vm["humidity"] is not None:
        detail = f"{detail} · {vm['humidity']}%" if detail else f"{vm['humidity']}%"
    vm["detail"] = detail
    return vm


def info_payload(hass: HomeAssistant, settings: dict[str, Any]) -> dict[str, str]:
    """The `info` event fields for the clock (docs/protocol.md, section 7)."""
    condition = temperature = indoor = ""
    if (weather := settings.get("weather_entity")) and (
        state := hass.states.get(weather)
    ) is not None and state.state not in (STATE_UNAVAILABLE, STATE_UNKNOWN):
        condition = state.state
        value = _float(state.attributes.get("temperature"))
        if value is not None:
            unit = state.attributes.get("temperature_unit") or hass.config.units.temperature_unit
            temperature = f"{round(value)} {unit}"
    if sensor := settings.get("indoor_temperature_entity"):
        value = _sensor_temperature(hass, sensor)
        if value is not None:
            indoor = f"{value:.1f} {hass.config.units.temperature_unit}"
    return {
        "weather_condition": condition,
        "weather_temperature": temperature,
        "indoor_temperature": indoor,
    }
