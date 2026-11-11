"""Daily weather forecast for the screensaver (docs/protocol.md, section 7)."""

from __future__ import annotations

from datetime import date
import logging
from typing import Any

from homeassistant.const import STATE_UNAVAILABLE, STATE_UNKNOWN
from homeassistant.core import HomeAssistant
from homeassistant.exceptions import HomeAssistantError
from homeassistant.util import dt as dt_util

from .const import FORECAST_DAYS

_LOGGER = logging.getLogger(__name__)


def _int(value: Any) -> int | None:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return None
    return round(value)


def forecast_days(
    entries: list[dict[str, Any]], today: date | None = None
) -> list[dict[str, Any]]:
    """Up to FORECAST_DAYS days from today, from HA's daily forecast entries."""
    today = today or dt_util.now().date()
    days: list[dict[str, Any]] = []
    seen: set[date] = set()
    for entry in entries:
        when = dt_util.parse_datetime(str(entry.get("datetime") or ""))
        if when is None:
            continue
        day = dt_util.as_local(when).date()
        if day < today or day in seen:
            continue
        seen.add(day)
        days.append(
            {
                "date": day.isoformat(),
                # Like C's tm_wday: 0 = Sunday.
                "weekday": (day.weekday() + 1) % 7,
                "condition": str(entry.get("condition") or ""),
                "high": _int(entry.get("temperature")),
                "low": _int(entry.get("templow")),
                "precipitation": _int(entry.get("precipitation_probability")),
            }
        )
        if len(days) == FORECAST_DAYS:
            break
    return days


async def async_forecast(hass: HomeAssistant, weather_entity: str) -> dict[str, Any]:
    """The `forecast` event fields: empty days when there is no forecast."""
    empty: dict[str, Any] = {"unit": "", "days": []}
    if not weather_entity:
        return empty
    state = hass.states.get(weather_entity)
    if state is None or state.state in (STATE_UNAVAILABLE, STATE_UNKNOWN):
        return empty
    try:
        response = await hass.services.async_call(
            "weather",
            "get_forecasts",
            {"entity_id": weather_entity, "type": "daily"},
            blocking=True,
            return_response=True,
        )
    except (HomeAssistantError, ValueError) as err:
        # For example a weather entity without a daily forecast.
        _LOGGER.debug("No daily forecast from %s: %s", weather_entity, err)
        return empty
    entries = ((response or {}).get(weather_entity) or {}).get("forecast") or []
    unit = state.attributes.get("temperature_unit") or hass.config.units.temperature_unit
    return {"unit": unit, "days": forecast_days(list(entries))}
