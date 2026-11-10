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
from homeassistant.util import dt as dt_util
from homeassistant.util.unit_conversion import TemperatureConverter

from .const import KIND_VALVE

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

    if value == STATE_UNKNOWN and domain not in ("button", "climate"):
        vm["status"] = "Unknown"
        return vm

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
            vm["detail"] = f"{int(position)}%"
        return vm

    if domain == "climate":
        return _climate(hass, vm, state, tile)

    vm["status"] = value.capitalize()
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
