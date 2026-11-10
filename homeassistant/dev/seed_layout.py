"""Save a starter layout for the real screen (works before it has registered).

    uv run --with aiohttp homeassistant/dev/seed_layout.py [--url ...] [--screen-id MAC]

Uses the dev HA's demo/template entities. Overwrites the screen's layout.
"""

from __future__ import annotations

import argparse
import asyncio
import sys
from pathlib import Path

import aiohttp

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "tests"))
sys.path.insert(0, str(Path(__file__).resolve().parent))
import local  # noqa: E402
from device_sim import HAWebSocket, login_flow  # noqa: E402


def tile(
    tid: str, kind: str, entity: str, x: int, y: int, w: int = 1, h: int = 1, label: str = "",
    temperature_entity: str = "", humidity_entity: str = "",
) -> dict:
    return {
        "id": tid, "kind": kind, "entity_id": entity, "label": label, "x": x, "y": y, "w": w, "h": h,
        "temperature_entity": temperature_entity, "humidity_entity": humidity_entity,
    }


LAYOUT = {
    "version": 1,
    "pages": [
        {
            "id": "home", "title": "Living room", "icon": "mdi:sofa", "cols": 4, "rows": 3,
            "tiles": [
                tile("t1", "light", "light.porch_light", 0, 0),
                tile("t2", "light", "light.ceiling_lights", 1, 0),
                tile("t3", "garage", "cover.garage_door", 2, 0),
                tile("t4", "gate", "cover.front_gate", 3, 0),
                tile("t5", "valve", "valve.front_garden", 0, 1),
                tile("t6", "climate", "climate.hvac", 1, 1, 2, 2,
                     temperature_entity="sensor.living_room_temperature",
                     humidity_entity="sensor.living_room_humidity"),
                tile("x1", "gate", "switch.gate_relay", 3, 1),
                tile("x2", "valve", "switch.gate_relay", 3, 2, label="Garden pump"),
            ],
        },
        {
            "id": "climate", "title": "Climate", "icon": "mdi:thermostat", "cols": 2, "rows": 1,
            "tiles": [
                tile("t7", "climate", "climate.hvac", 0, 0),
                tile("t8", "climate", "climate.heatpump", 1, 0),
            ],
        },
        {
            "id": "automations", "title": "Automations", "icon": "mdi:robot", "cols": 3, "rows": 2,
            "tiles": [
                tile("a1", "toggle", "automation.night_lights", 0, 0),
                tile("a2", "toggle", "input_boolean.guest_mode", 1, 0),
                tile("a3", "toggle", "switch.gate_relay", 2, 0),
            ],
        },
    ],
}


async def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--url", default="http://localhost:8123")
    parser.add_argument("--screen-id", default=local.real_screen_id(),
                        help="screen MAC (default: real_screen_id in dev-config/dev-local.json)")
    parser.add_argument("--user", default=local.admin()[0])
    parser.add_argument("--password", default=local.admin()[1])
    args = parser.parse_args()
    if not args.screen_id:
        parser.error("no --screen-id given and no real_screen_id in dev-config/dev-local.json")
    url = args.url.rstrip("/")
    async with aiohttp.ClientSession() as session:
        tokens = await login_flow(session, url, f"{url}/", args.user, args.password)
        ws = HAWebSocket(session, url)
        await ws.connect(tokens["access_token"])
        await ws.call("esp32_screen/layout/save", screen_id=args.screen_id, layout=LAYOUT)
        screens = (await ws.call("esp32_screen/screens"))["screens"]
        rec = next(s for s in screens if s["screen_id"] == args.screen_id)
        print(f"saved layout for {rec['screen_id']} ({rec['name']}): "
              + ", ".join(f"{p['title']} {p['cols']}x{p['rows']} {len(p['tiles'])} tiles" for p in rec["layout"]["pages"]))
        await ws.close()


if __name__ == "__main__":
    asyncio.run(main())
