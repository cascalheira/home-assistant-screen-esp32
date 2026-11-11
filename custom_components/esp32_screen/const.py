"""Constants for the ESP32 Screen integration."""

from __future__ import annotations

from typing import Final

DOMAIN: Final = "esp32_screen"
MANUFACTURER: Final = "ESP32 Screen"

STORAGE_KEY: Final = DOMAIN
STORAGE_VERSION: Final = 1

PANEL_URL_PATH: Final = "esp32-screen"
PANEL_COMPONENT: Final = "esp32-screen-panel"
PANEL_TITLE: Final = "Screens"
PANEL_ICON: Final = "mdi:tablet-dashboard"
STATIC_URL: Final = "/esp32_screen_static"
PANEL_FILE: Final = "esp32-screen-panel.js"

# screen_id = MAC, 12 upper-case hex digits.
SCREEN_ID_PATTERN: Final = r"^[0-9A-F]{12}$"

LAYOUT_VERSION: Final = 1
MAX_PAGES: Final = 6
MAX_COLS: Final = 8
MAX_ROWS: Final = 6
MAX_ID_LEN: Final = 32
MAX_TITLE_LEN: Final = 32
MAX_LABEL_LEN: Final = 48
MAX_NAME_LEN: Final = 64

KIND_LIGHT: Final = "light"
KIND_GATE: Final = "gate"
KIND_GARAGE: Final = "garage"
KIND_VALVE: Final = "valve"
KIND_CLIMATE: Final = "climate"
KIND_TOGGLE: Final = "toggle"
KIND_SCENE: Final = "scene"
KIND_SENSOR: Final = "sensor"
KIND_BLIND: Final = "blind"
KIND_LOCK: Final = "lock"
KIND_ALARM: Final = "alarm"
KIND_MEDIA: Final = "media"

KIND_DOMAINS: Final[dict[str, tuple[str, ...]]] = {
    KIND_LIGHT: ("light", "switch"),
    KIND_GATE: ("cover", "switch", "button"),
    KIND_GARAGE: ("cover", "switch", "button"),
    KIND_VALVE: ("valve", "switch"),
    KIND_CLIMATE: ("climate",),
    KIND_TOGGLE: ("automation", "input_boolean", "switch"),
    KIND_SCENE: ("scene", "script", "button", "input_button"),
    KIND_SENSOR: ("sensor", "binary_sensor"),
    KIND_BLIND: ("cover",),
    KIND_LOCK: ("lock",),
    KIND_ALARM: ("alarm_control_panel",),
    KIND_MEDIA: ("media_player",),
}

KIND_ACTIONS: Final[dict[str, tuple[str, ...]]] = {
    KIND_LIGHT: ("toggle", "brightness", "hue", "kelvin"),
    KIND_GATE: ("toggle", "open", "close", "stop"),
    KIND_GARAGE: ("toggle", "open", "close", "stop"),
    KIND_VALVE: ("toggle", "open", "close"),
    KIND_CLIMATE: ("toggle", "set_temperature", "set_mode"),
    KIND_TOGGLE: ("toggle", "on", "off"),
    KIND_SCENE: ("activate", "toggle"),
    KIND_SENSOR: (),
    KIND_BLIND: ("toggle", "open", "close", "stop", "position"),
    KIND_LOCK: ("toggle", "lock", "unlock"),
    KIND_ALARM: ("arm_home", "arm_away", "arm_night", "arm_vacation", "disarm"),
    KIND_MEDIA: ("toggle", "play_pause", "next", "previous", "volume"),
}

# Alarm arm modes in `modes` order, with the alarm_control_panel feature bit.
ALARM_MODES: Final[tuple[tuple[str, int], ...]] = (
    ("arm_home", 1),
    ("arm_away", 2),
    ("arm_night", 4),
    ("arm_vacation", 32),
)

# Page icons the firmware can draw (glyphs compiled in), in picker order.
PAGE_ICONS: Final[tuple[str, ...]] = (
    "mdi:home", "mdi:sofa", "mdi:bed", "mdi:silverware-fork-knife", "mdi:coffee",
    "mdi:shower", "mdi:bathtub", "mdi:toilet", "mdi:desk", "mdi:wardrobe-outline",
    "mdi:baby-face-outline", "mdi:toy-brick-outline", "mdi:washing-machine",
    "mdi:stairs", "mdi:door", "mdi:home-floor-0", "mdi:home-floor-1",
    "mdi:home-floor-2", "mdi:home-floor-b", "mdi:home-roof", "mdi:balcony",
    "mdi:garage", "mdi:car", "mdi:gate", "mdi:tree", "mdi:flower", "mdi:sprout",
    "mdi:pool", "mdi:paw", "mdi:dumbbell", "mdi:television", "mdi:music",
    "mdi:gamepad-variant", "mdi:lightbulb-group", "mdi:thermostat",
    "mdi:air-conditioner", "mdi:sun-snowflake-variant", "mdi:fan", "mdi:fire",
    "mdi:snowflake", "mdi:water", "mdi:power-plug", "mdi:weather-sunny",
    "mdi:robot", "mdi:shield-home", "mdi:cctv",
)

# Icons a light tile can show instead of the bulb (glyphs compiled in), in
# picker order. "" = the default bulb.
LIGHT_ICONS: Final[tuple[str, ...]] = (
    "mdi:ceiling-light", "mdi:ceiling-light-multiple", "mdi:chandelier",
    "mdi:light-recessed", "mdi:track-light", "mdi:spotlight-beam", "mdi:lightbulb-spot",
    "mdi:dome-light", "mdi:vanity-light", "mdi:wall-sconce", "mdi:wall-sconce-flat",
    "mdi:wall-sconce-round", "mdi:coach-lamp", "mdi:coach-lamp-variant",
    "mdi:outdoor-lamp", "mdi:post-lamp", "mdi:bulkhead-light", "mdi:light-flood-down",
    "mdi:light-flood-up", "mdi:globe-light", "mdi:lamp", "mdi:lamps", "mdi:desk-lamp",
    "mdi:floor-lamp", "mdi:floor-lamp-torchiere", "mdi:lightbulb-fluorescent-tube",
    "mdi:led-strip-variant", "mdi:string-lights", "mdi:television-ambient-light",
    "mdi:ceiling-fan-light", "mdi:lightbulb-group",
)

# Light tile options (docs/protocol.md, section 2). The first is the default.
LIGHT_TAP: Final = ("toggle", "controls")
LIGHT_INLINE: Final = ("", "brightness", "color", "brightness_color")

# Optional sensor fields of a tile; only climate tiles may set them.
SENSOR_FIELDS: Final = ("temperature_entity", "humidity_entity")
SENSOR_DOMAIN: Final = "sensor"

ERR_INVALID_LAYOUT: Final = "invalid_layout"
ERR_INVALID_ACTION: Final = "invalid_action"
ERR_SERVICE_FAILED: Final = "service_failed"
ERR_NOT_LOADED: Final = "not_loaded"

# Firmware updates (docs/protocol.md, section 5).
FIRMWARE_DIR: Final = "esp32_screen/firmware"  # relative to the HA config dir
FIRMWARE_PROJECT: Final = "esp32-screen"
FIRMWARE_URL: Final = "/api/esp32_screen/firmware"
FIRMWARE_SCAN_INTERVAL: Final = 60  # seconds
OTA_TIMEOUT: Final = 600  # seconds
OTA_STATES: Final = ("downloading", "verifying", "restarting", "failed")

# Screen controls (docs/protocol.md, section 6).
SCREENSAVER_MODES: Final = ("off", "clock", "flip")
# Seconds between clock and forecast when both are on.
SCREENSAVER_ROTATE: Final = (10, 20, 30, 60)
COMMANDS: Final = (
    "brightness", "screensaver_mode", "screensaver_delay", "screensaver_weather",
    "screensaver_rotate", "wake", "sleep",
)
# Weather forecast for the screensaver (docs/protocol.md, section 7).
FORECAST_DAYS: Final = 7
FORECAST_REFRESH: Final = 30 * 60  # seconds
FORECAST_MIN_INTERVAL: Final = 5 * 60  # seconds between fetches on state changes

# Per-screen settings (docs/protocol.md, section 7).
LANGUAGES: Final = ("en", "pt")


def default_settings() -> dict[str, object]:
    """Settings of a screen nobody configured yet."""
    return {
        "weather_entity": "",
        "indoor_temperature_entity": "",
        "language": "en",
        "clock_24h": True,
    }
