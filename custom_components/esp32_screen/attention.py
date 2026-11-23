"""The "at a glance" line on the screensaver (docs/protocol.md, section 7).

Each screen has up to MAX_ATTENTION_RULES rules. A rule names an entity and
when it needs attention: one of `states`, below or above a number, or, with
none of those, a default for the entity's domain (an open door, a light on).
Active rules become `{"icon", "text"}` items for the screen.
"""

from __future__ import annotations

from typing import Any

from homeassistant.const import STATE_UNAVAILABLE, STATE_UNKNOWN
from homeassistant.core import HomeAssistant, State

from .const import MAX_ATTENTION_ITEMS
from .i18n import localize
from .view_model import _float, resolve_label, tile_view_model

# States that need attention when a rule gives no states and no threshold.
DEFAULT_STATES: dict[str, tuple[str, ...]] = {
    "binary_sensor": ("on",),
    "cover": ("open", "opening"),
    "valve": ("open", "opening"),
    "lock": ("unlocked", "open", "opening", "jammed"),
    "light": ("on",),
    "switch": ("on",),
    "fan": ("on",),
    "input_boolean": ("on",),
    "media_player": ("on", "playing"),
    "alarm_control_panel": ("triggered",),
}

# Domains whose numeric state is shown like a sensor's (value and unit).
_NUMERIC_DOMAINS = ("sensor", "binary_sensor", "input_number", "number")

_BINARY_ICONS: dict[str, str] = {
    "door": "door",
    "opening": "door",
    "window": "window",
    "garage_door": "garage",
    "lock": "lock",
    "motion": "motion",
    "occupancy": "motion",
    "presence": "motion",
    "moisture": "water",
    "smoke": "smoke",
    "gas": "smoke",
    "carbon_monoxide": "smoke",
    "battery": "battery",
    "heat": "thermometer",
    "cold": "thermometer",
    "light": "light",
    "plug": "power",
    "power": "power",
}

_COVER_ICONS: dict[str, str] = {
    "garage": "garage",
    "gate": "garage",
    "door": "door",
    "window": "window",
    "blind": "window",
    "shutter": "window",
    "curtain": "window",
    "shade": "window",
    "awning": "window",
}

_DOMAIN_ICONS: dict[str, str] = {
    "lock": "lock",
    "light": "light",
    "switch": "power",
    "fan": "power",
    "input_boolean": "power",
    "media_player": "power",
    "valve": "water",
}

_SENSOR_ICONS: dict[str, str] = {
    "battery": "battery",
    "temperature": "thermometer",
    "moisture": "water",
}


def needs_condition(entity_id: str) -> bool:
    """True when a rule for this entity must give states or a threshold."""
    return entity_id.split(".", 1)[0] not in DEFAULT_STATES


def rule_active(rule: dict[str, Any], state: State | None) -> bool:
    """Whether the rule's entity needs attention now."""
    if state is None or state.state in (STATE_UNAVAILABLE, STATE_UNKNOWN):
        return False
    states = [str(s).strip().lower() for s in rule.get("states") or [] if str(s).strip()]
    below = rule.get("below")
    above = rule.get("above")
    current = state.state.lower()
    if states and current in states:
        return True
    if below is not None or above is not None:
        value = _float(state.state)
        if value is not None and (
            (below is not None and value < below) or (above is not None and value > above)
        ):
            return True
    if not states and below is None and above is None:
        return current in DEFAULT_STATES.get(state.domain, ())
    return False


def attention_icon(domain: str, device_class: str | None) -> str:
    """One of const.ATTENTION_ICONS for an entity."""
    device_class = device_class or ""
    if domain == "binary_sensor":
        return _BINARY_ICONS.get(device_class, "alert")
    if domain == "cover":
        return _COVER_ICONS.get(device_class, "garage")
    if domain == "sensor":
        return _SENSOR_ICONS.get(device_class, "alert")
    return _DOMAIN_ICONS.get(domain, "alert")


def attention_text(label: str, status: str) -> str:
    """'<label> · <status>', or the label alone without a status."""
    return f"{label} · {status}" if status else label


def status_text(hass: HomeAssistant, state: State, language: str) -> str:
    """The entity's state as a tile shows it ("Open", "12 %"), in `language`."""
    kind = "sensor" if state.domain in _NUMERIC_DOMAINS else ""
    tile = {"id": "", "kind": kind, "entity_id": state.entity_id, "label": ""}
    vm = localize(tile_view_model(hass, tile, state), language)
    return str(vm.get("status") or "")


def attention_items(
    hass: HomeAssistant, rules: list[dict[str, Any]], language: str
) -> list[dict[str, str]]:
    """The `attention` event items: active rules in order, at most MAX_ATTENTION_ITEMS."""
    items: list[dict[str, str]] = []
    for rule in rules:
        state = hass.states.get(rule.get("entity_id") or "")
        if not rule_active(rule, state):
            continue
        assert state is not None
        label = resolve_label(hass, {"label": rule.get("label") or "", "entity_id": state.entity_id})
        items.append(
            {
                "icon": attention_icon(state.domain, state.attributes.get("device_class")),
                "text": attention_text(label, status_text(hass, state, language)),
            }
        )
        if len(items) >= MAX_ATTENTION_ITEMS:
            break
    return items
