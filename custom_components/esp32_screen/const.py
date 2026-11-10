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
    KIND_LIGHT: ("toggle", "brightness"),
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
