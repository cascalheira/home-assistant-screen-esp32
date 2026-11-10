"""Local settings of the development Home Assistant (not in git).

Kept in `homeassistant/dev-config/dev-local.json`, next to the dev instance's
own config, which git ignores. `onboard.py` creates it with random passwords
on first use. Environment variables override each value:

    ESP32_SCREEN_ADMIN_USER / ESP32_SCREEN_ADMIN_PASSWORD   admin (owner) user
    ESP32_SCREEN_USER / ESP32_SCREEN_PASSWORD               the screens' non-admin user
    ESP32_SCREEN_REAL_ID                                    MAC of a real screen (optional)
"""

from __future__ import annotations

import json
import os
import secrets
from pathlib import Path

FILE = Path(__file__).resolve().parent.parent / "dev-config" / "dev-local.json"


def _stored() -> dict:
    try:
        return json.loads(FILE.read_text())
    except (OSError, ValueError):
        return {}


def ensure() -> dict:
    """Loads the settings, creating the file with random passwords if needed."""
    data = _stored()
    changed = False
    defaults = {
        "admin_user": "dev",
        "admin_password": secrets.token_urlsafe(18),
        "screen_user": "screen",
        "screen_password": secrets.token_urlsafe(18),
        "real_screen_id": "",
    }
    for key, value in defaults.items():
        if key not in data:
            data[key] = value
            changed = True
    if changed:
        FILE.parent.mkdir(parents=True, exist_ok=True)
        FILE.write_text(json.dumps(data, indent=2) + "\n")
        FILE.chmod(0o600)
    return data


def _get(key: str, env: str) -> str:
    return os.environ.get(env) or str(ensure().get(key, ""))


def admin() -> tuple[str, str]:
    return _get("admin_user", "ESP32_SCREEN_ADMIN_USER"), _get("admin_password", "ESP32_SCREEN_ADMIN_PASSWORD")


def screen_user() -> tuple[str, str]:
    return _get("screen_user", "ESP32_SCREEN_USER"), _get("screen_password", "ESP32_SCREEN_PASSWORD")


def real_screen_id() -> str:
    """MAC of a real screen registered on the dev instance, or "" if none."""
    return _get("real_screen_id", "ESP32_SCREEN_REAL_ID").upper()
