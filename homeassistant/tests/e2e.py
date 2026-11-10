"""End-to-end test of the esp32_screen integration against a running HA.

    uv run --with aiohttp homeassistant/tests/e2e.py [--url http://localhost:8123]

Needs the dev HA from homeassistant/dev/ (demo entities + the template
entities in dev/configuration.yaml), the admin `dev` and the non-admin `screen`.
Prints PASS/FAIL per check and exits non-zero on any failure.
"""

from __future__ import annotations

import argparse
import asyncio
import copy
import hashlib
import os
import struct
import sys
from pathlib import Path
from typing import Any

import aiohttp

sys.path.insert(0, str(Path(__file__).parent))
from device_sim import VM_KEYS, CommandError, HAWebSocket, ScreenSim, login_flow, refresh_token

SIM_ID = "AABBCCDDEEFF"
# The page icons the firmware can draw, as specified (docs/protocol.md).
ALLOWED_ICONS = """
mdi:home mdi:sofa mdi:bed mdi:silverware-fork-knife mdi:coffee mdi:shower mdi:bathtub mdi:toilet mdi:desk
mdi:wardrobe-outline mdi:baby-face-outline mdi:toy-brick-outline mdi:washing-machine mdi:stairs mdi:door
mdi:home-floor-0 mdi:home-floor-1 mdi:home-floor-2 mdi:home-floor-b mdi:home-roof mdi:balcony mdi:garage
mdi:car mdi:gate mdi:tree mdi:flower mdi:sprout mdi:pool mdi:paw mdi:dumbbell mdi:television mdi:music
mdi:gamepad-variant mdi:lightbulb-group mdi:thermostat mdi:air-conditioner mdi:sun-snowflake-variant mdi:fan
mdi:fire mdi:snowflake mdi:water mdi:power-plug mdi:weather-sunny mdi:robot mdi:shield-home mdi:cctv
""".split()
TEMP_ID = "112233445566"
sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "dev"))
import local  # noqa: E402

ADMIN = local.admin()
SCREEN = local.screen_user()
REAL_ID = local.real_screen_id()  # "" when no real screen is registered

RESULTS: list[tuple[bool, str, str]] = []
DEFAULT_FIRMWARE_DIR = Path(__file__).resolve().parent.parent / "dev-config" / "esp32_screen" / "firmware"
FW_PREFIX = "e2e-test-"


def fake_image(version: str, project: str = "esp32-screen", magic: int = 0xABCD5432, size: int = 150_000) -> bytes:
    """An ESP-IDF app image header with an esp_app_desc_t; the rest is noise."""
    buf = bytearray(os.urandom(size))
    buf[0] = 0xE9  # image magic
    struct.pack_into("<I", buf, 32, magic)
    buf[48:80] = version.encode().ljust(32, b"\0")
    buf[80:112] = project.encode().ljust(32, b"\0")
    return bytes(buf)


async def firmware_tests(
    admin: HAWebSocket, sim: ScreenSim, firmware_dir: Path, session: aiohttp.ClientSession, url: str
) -> None:
    """Section 5: firmware folder, update entity, download, install, ota_status."""
    registry = await admin.call("config/entity_registry/list")
    entity_id = next((e["entity_id"] for e in registry if e.get("unique_id") == f"{SIM_ID}_firmware"), None)
    real_entity = next((e["entity_id"] for e in registry if REAL_ID and e.get("unique_id") == f"{REAL_ID}_firmware"), None)
    check(entity_id is not None, "an update entity exists for the screen", str(entity_id))
    if entity_id is None:
        return

    async def entity_state() -> dict[str, Any]:
        return next(st for st in await admin.call("get_states") if st["entity_id"] == entity_id)

    async def refresh(eid: str | None = None) -> None:
        await admin.call("call_service", domain="homeassistant", service="update_entity",
                         target={"entity_id": eid or entity_id}, service_data={})

    async def install() -> Any:
        return await admin.call("call_service", domain="update", service="install",
                                target={"entity_id": entity_id}, service_data={})

    await sim.ws.call("esp32_screen/register", screen_id=SIM_ID, name="Sim Screen", model="ESP32-8048S043", sw_version="0.2.0")

    firmware_dir.mkdir(parents=True, exist_ok=True)
    images = {
        "0.8.5": fake_image("0.8.5"),
        "0.10.0": fake_image("0.10.0"),  # newest: 0.10.0 > 0.9.x needs a real version compare
        "0.9.1": fake_image("0.9.1"),
    }
    paths = []
    for version, data in images.items():
        path = firmware_dir / f"{FW_PREFIX}{version}.bin"
        path.write_bytes(data)
        paths.append(path)
    bad = {
        "bad-magic": fake_image("9.0.0", magic=0x12345678),
        "other-project": fake_image("9.9.0", project="other-project"),
        "short": b"\xe9" * 40,
    }
    for name, data in bad.items():
        path = firmware_dir / f"{FW_PREFIX}{name}.bin"
        path.write_bytes(data)
        paths.append(path)
    latest = images["0.10.0"]
    sha = hashlib.sha256(latest).hexdigest()
    try:
        await refresh()
        st = await entity_state()
        attrs = st["attributes"]
        check(attrs["latest_version"] == "0.10.0" and attrs["installed_version"] == "0.2.0" and st["state"] == "on"
              and attrs["title"] == "ESP32 Screen firmware",
              "update entity: latest = highest valid image, invalid images ignored",
              f"state={st['state']} installed={attrs['installed_version']} latest={attrs['latest_version']}")

        # Download.
        status, body, headers = await sim.download(f"/api/esp32_screen/firmware/{sha}.bin")
        check(status == 200 and hashlib.sha256(body).hexdigest() == sha and int(headers.get("Content-Length", -1)) == len(latest)
              and headers.get("Content-Type", "").startswith("application/octet-stream"),
              "download with the screen user's token", f"HTTP {status}, {len(body)} bytes, {headers.get('Content-Type')}")
        status, _, _ = await sim.download(f"/api/esp32_screen/firmware/{sha}.bin", token=None)
        check(status == 401, "download without a token -> 401", f"HTTP {status}")
        status, _, _ = await sim.download(f"/api/esp32_screen/firmware/{'0' * 64}.bin")
        check(status == 404, "download of an unknown sha256 -> 404", f"HTTP {status}")
        other_sha = hashlib.sha256(bad["other-project"]).hexdigest()
        status, _, _ = await sim.download(f"/api/esp32_screen/firmware/{other_sha}.bin")
        check(status == 404, "download of an ignored (invalid) image -> 404", f"HTTP {status}")

        # Install: ota event, progress, restarting.
        mark = sim.mark()
        task = asyncio.create_task(install())
        ev = await sim.wait_for(lambda e: e.get("kind") == "ota", 10, since=mark)
        check(ev == {"kind": "ota", "version": "0.10.0", "size": len(latest), "sha256": sha,
                     "url": f"/api/esp32_screen/firmware/{sha}.bin"}, "install sends the ota event", str(ev)[:160])
        await sim.ota_status("downloading", 40)
        await asyncio.sleep(0.3)
        attrs = (await entity_state())["attributes"]
        check(attrs["in_progress"] is True and attrs["update_percentage"] == 40,
              "ota_status progress shows on the update entity",
              f"in_progress={attrs['in_progress']} update_percentage={attrs['update_percentage']}")
        await sim.ota_status("verifying", 100)
        await sim.ota_status("restarting", 100)
        try:
            result = await asyncio.wait_for(task, 10)
            check(True, "'restarting' finishes the install successfully", str(result)[:80])
        except (CommandError, TimeoutError) as err:
            check(False, "'restarting' finishes the install successfully", str(err))
        attrs = (await entity_state())["attributes"]
        check(attrs["in_progress"] is False, "in_progress is false after the install", str(attrs["in_progress"]))

        # Install that fails.
        mark = sim.mark()
        task = asyncio.create_task(install())
        await sim.wait_for(lambda e: e.get("kind") == "ota", 10, since=mark)
        await sim.ota_status("downloading", 10)
        await sim.ota_status("failed", 10, "sha256 mismatch")
        try:
            await asyncio.wait_for(task, 10)
            check(False, "'failed' makes the install fail with the screen's error", "install succeeded")
        except CommandError as err:
            check("sha256 mismatch" in err.message, "'failed' makes the install fail with the screen's error", str(err))

        await expect_error(sim.ws.call("esp32_screen/ota_status", screen_id="000000000000", state="downloading", progress=1),
                           "not_found", "ota_status for an unknown screen -> not_found")
        await expect_error(sim.ws.call("esp32_screen/ota_status", screen_id=SIM_ID, state="flashing", progress=1),
                           "invalid_format", "ota_status with an unknown state -> invalid_format")

        # The screen restarts with the new version and registers again.
        await sim.ws.call("esp32_screen/register", screen_id=SIM_ID, name="Sim Screen", model="ESP32-8048S043", sw_version="0.10.0")
        await asyncio.sleep(0.3)
        st = await entity_state()
        check(st["state"] == "off" and st["attributes"]["installed_version"] == "0.10.0",
              "register with the new sw_version -> installed_version updated, no update pending",
              f"state={st['state']} installed={st['attributes']['installed_version']}")

        # A simulated device doing the whole update by itself (device_sim --ota apply).
        await sim.ws.call("esp32_screen/register", screen_id=SIM_ID, name="Sim Screen", model="ESP32-8048S043", sw_version="0.2.0")
        sim.ota_mode = "apply"
        try:
            await asyncio.wait_for(install(), 15)
            await sim.wait_for(lambda e: e.get("kind") == "layout", 15, since=sim.mark())
            await asyncio.sleep(0.5)
            st = await entity_state()
            check(st["state"] == "off" and st["attributes"]["installed_version"] == "0.10.0",
                  "simulated device: download + verify + restart + re-register end to end",
                  f"state={st['state']} installed={st['attributes']['installed_version']}")
        except (CommandError, TimeoutError) as err:
            check(False, "simulated device: download + verify + restart + re-register end to end", str(err))
        finally:
            sim.ota_mode = "ignore"

        # Offline screen.
        await sim.ws.call("esp32_screen/register", screen_id=SIM_ID, name="Sim Screen", model="ESP32-8048S043", sw_version="0.2.0")
        await sim.stop()
        await asyncio.sleep(0.3)
        await expect_error(install(), "home_assistant_error", "install on an offline screen fails", "Screen is offline")
        await sim.start()
        await sim.ws.call("esp32_screen/register", screen_id=SIM_ID, name="Sim Screen", model="ESP32-8048S043", sw_version="9.9.9")
    finally:
        for path in paths:
            path.unlink(missing_ok=True)
        await refresh()
        if real_entity:
            await refresh(real_entity)
    st = await entity_state()
    # Real images published to the folder (dist/*.bin) may remain; only the
    # fake ones must be gone.
    check(st["attributes"]["latest_version"] not in images, "fake images removed again; none offered",
          f"latest={st['attributes']['latest_version']}")


def check(ok: bool, name: str, detail: str = "") -> bool:
    RESULTS.append((ok, name, detail))
    print(f"{'PASS' if ok else 'FAIL'}  {name}" + (f"  ({detail})" if detail else ""), flush=True)
    return ok


async def expect_error(coro: Any, code: str, name: str, contains: str = "") -> None:
    try:
        await coro
    except CommandError as err:
        ok = err.code == code and contains.lower() in err.message.lower()
        check(ok, name, f"{err.code}: {err.message}")
        return
    check(False, name, "no error")


def tile(
    tid: str, kind: str, entity: str, x: int, y: int, w: int = 1, h: int = 1, label: str = "",
    temperature_entity: str = "", humidity_entity: str = "",
) -> dict[str, Any]:
    return {
        "id": tid, "kind": kind, "entity_id": entity, "label": label, "x": x, "y": y, "w": w, "h": h,
        "temperature_entity": temperature_entity, "humidity_entity": humidity_entity,
    }


TEST_LAYOUT: dict[str, Any] = {
    "version": 1,
    "pages": [
        {
            "id": "p1", "title": "Test", "icon": "mdi:home", "cols": 4, "rows": 3,
            "tiles": [
                tile("t1", "light", "light.ceiling_lights", 0, 0),
                tile("t2", "light", "light.porch_light", 1, 0),
                tile("t3", "gate", "cover.front_gate", 2, 0),
                tile("t4", "garage", "cover.garage_door", 3, 0),
                tile("t5", "valve", "valve.orchard", 0, 1),
                tile("t6", "climate", "climate.hvac", 1, 1, 2, 1),
                tile("t7", "gate", "button.push", 3, 1),
                tile("t8", "gate", "cover.hall_window", 0, 2),
                tile("t9", "light", "switch.decorative_lights", 1, 2),
                tile("t10", "valve", "switch.ac", 2, 2, label="Drip line"),
            ],
        },
        {
            # No "icon" key on purpose: it must default to "".
            "id": "p2", "title": "More", "cols": 4, "rows": 2,
            "tiles": [
                tile("t11", "toggle", "automation.night_lights", 0, 0),
                tile("t12", "toggle", "input_boolean.guest_mode", 1, 0),
                tile("t13", "toggle", "switch.gate_relay", 2, 0),
                tile("t16", "toggle", "automation.gate_opened", 3, 0),
                tile("t14", "climate", "climate.hvac", 0, 1, 2, 1, label="Room",
                     temperature_entity="sensor.living_room_temperature",
                     humidity_entity="sensor.living_room_humidity"),
                tile("t15", "climate", "climate.heatpump", 2, 1, 2, 1,
                     temperature_entity="sensor.attic_temperature_f"),
            ],
        },
    ],
}
ALL_TILES = {t["id"] for p in TEST_LAYOUT["pages"] for t in p["tiles"]}


async def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--url", default="http://localhost:8123")
    parser.add_argument("--firmware-dir", type=Path, default=DEFAULT_FIRMWARE_DIR,
                        help="host path of <HA config>/esp32_screen/firmware")
    args = parser.parse_args()
    url = args.url.rstrip("/")
    firmware_dir: Path = args.firmware_dir

    async with aiohttp.ClientSession() as session:
        admin_tokens = await login_flow(session, url, f"{url}/", *ADMIN)
        admin = HAWebSocket(session, url)
        await admin.connect(admin_tokens["access_token"])

        screen_lists: list[list[dict[str, Any]]] = []
        await admin.subscribe(lambda ev: screen_lists.append(ev["screens"]), "esp32_screen/subscribe_screens")
        await asyncio.sleep(0.3)
        check(len(screen_lists) >= 1, "subscribe_screens sends the current list right away")

        # Clean slate for the test screens.
        for sid in (SIM_ID, TEMP_ID):
            try:
                await admin.call("esp32_screen/screen/remove", screen_id=sid)
            except CommandError:
                pass

        # ---- server-side validation -------------------------------------------------
        def broken(mutate: Any) -> dict[str, Any]:
            layout = copy.deepcopy(TEST_LAYOUT)
            mutate(layout)
            return layout

        page = lambda lay: lay["pages"][0]  # noqa: E731
        cases = [
            ("overlap", lambda lay: page(lay)["tiles"].append(tile("t99", "light", "light.bed_light", 1, 1)), "overlaps"),
            ("out of bounds", lambda lay: page(lay)["tiles"][3].update(w=2), "width"),
            ("negative x", lambda lay: page(lay)["tiles"][0].update(x=-1), "between"),
            ("wrong domain", lambda lay: page(lay)["tiles"][5].update(entity_id="light.bed_light"), "cannot use"),
            ("unknown kind", lambda lay: page(lay)["tiles"][0].update(kind="fan"), "kind"),
            ("duplicate tile id", lambda lay: page(lay)["tiles"][1].update(id="t1"), "already used"),
            ("duplicate page id", lambda lay: lay["pages"].append({**copy.deepcopy(page(lay)), "tiles": []}), "used twice"),
            ("cols > 8", lambda lay: page(lay).update(cols=9), "columns"),
            ("rows > 6", lambda lay: page(lay).update(rows=7), "rows"),
            ("7 pages", lambda lay: lay["pages"].extend({"id": f"x{i}", "title": f"P{i}", "cols": 1, "rows": 1, "tiles": []} for i in range(6)), "at most 6"),
            ("empty title", lambda lay: page(lay).update(title="  "), "empty"),
            ("missing entity", lambda lay: page(lay)["tiles"][0].update(entity_id=""), "pick an entity"),
            ("toggle with a light", lambda lay: lay["pages"][1]["tiles"][0].update(entity_id="light.bed_light"), "cannot use"),
            ("temperature_entity not a sensor", lambda lay: page(lay)["tiles"][5].update(temperature_entity="light.bed_light"), "sensor.*"),
            ("humidity_entity malformed", lambda lay: page(lay)["tiles"][5].update(humidity_entity="not an id"), "sensor.*"),
            ("temperature_entity not text", lambda lay: page(lay)["tiles"][5].update(temperature_entity=5), "must be text"),
            ("icon not in the allowed list", lambda lay: page(lay).update(icon="mdi:lightbulb"), "icon"),
            ("icon without mdi: prefix", lambda lay: page(lay).update(icon="home"), "icon"),
            ("icon not text", lambda lay: page(lay).update(icon=7), "icon"),
            ("sensor on a non-climate tile", lambda lay: page(lay)["tiles"][0].update(humidity_entity="sensor.living_room_humidity"), "only allowed on climate"),
        ]
        for name, mutate, contains in cases:
            await expect_error(
                admin.call("esp32_screen/layout/save", screen_id=SIM_ID, layout=broken(mutate)),
                "invalid_layout", f"layout/save rejects {name}", contains,
            )
        await expect_error(
            admin.call("esp32_screen/layout/save", screen_id="aabbccddeeff", layout=TEST_LAYOUT),
            "invalid_format", "layout/save rejects a lower-case screen_id",
        )

        # ---- every stored layout (incl. ones saved before page icons) has page icons --------
        stored = (await admin.call("esp32_screen/screens"))["screens"]
        check(all("icon" in p for s_ in stored for p in s_["layout"]["pages"]),
              "every page of every stored layout has an icon field",
              str({s_["screen_id"]: [p.get("icon") for p in s_["layout"]["pages"]] for s_ in stored}))

        # ---- all 46 allowed icons are accepted -------------------------------------------------
        allowed = ALLOWED_ICONS
        icon_layout: dict[str, Any] = {"version": 1, "pages": []}
        ok_all = True
        for k in range(0, len(allowed), 6):
            icon_layout["pages"] = [
                {"id": f"i{n}", "title": f"I{n}", "icon": icon, "cols": 1, "rows": 1, "tiles": []}
                for n, icon in enumerate(allowed[k:k + 6])
            ]
            try:
                await admin.call("esp32_screen/layout/save", screen_id=TEMP_ID, layout=icon_layout)
            except CommandError as err:
                ok_all = False
                print("  rejected:", err)
        check(ok_all and len(allowed) == 46, f"all {len(allowed)} allowed page icons are accepted")
        await admin.call("esp32_screen/screen/remove", screen_id=TEMP_ID)

        # ---- save for a not-yet-registered screen ---------------------------------------
        await admin.call("esp32_screen/layout/save", screen_id=SIM_ID, layout=TEST_LAYOUT)
        screens = (await admin.call("esp32_screen/screens"))["screens"]
        rec = next((s for s in screens if s["screen_id"] == SIM_ID), None)
        check(rec is not None and rec["layout"]["pages"][0]["tiles"][9]["label"] == "Drip line" and not rec["online"],
              "layout/save creates the record of an unregistered screen", f"name={rec and rec['name']}")

        # ---- known starting states, so the run does not depend on earlier runs --------
        for domain, service, entity, data in (
            ("light", "turn_on", "light.ceiling_lights", {"brightness_pct": 70}),
            ("light", "turn_off", "light.porch_light", {}),
            ("switch", "turn_on", "switch.decorative_lights", {}),
            ("switch", "turn_off", "switch.ac", {}),
            ("cover", "close_cover", "cover.front_gate", {}),
            ("cover", "close_cover", "cover.garage_door", {}),
            ("valve", "close_valve", "valve.orchard", {}),
            ("climate", "set_hvac_mode", "climate.hvac", {"hvac_mode": "cool"}),
            ("climate", "set_temperature", "climate.hvac", {"temperature": 21}),
            ("input_number", "set_value", "input_number.room_temperature", {"value": 23.4}),
            ("input_number", "set_value", "input_number.room_humidity", {"value": 46.6}),
            ("input_number", "set_value", "input_number.attic_temperature_f", {"value": 68}),
            ("input_boolean", "turn_on", "input_boolean.room_sensor_online", {}),
            ("input_boolean", "turn_off", "input_boolean.guest_mode", {}),
            ("switch", "turn_off", "switch.gate_relay", {}),
            ("automation", "turn_on", "automation.night_lights", {}),
        ):
            await admin.call("call_service", domain=domain, service=service, target={"entity_id": entity},
                             service_data=data)
        await asyncio.sleep(3)  # demo valves take a moment

        # ---- the simulated screen ------------------------------------------------------
        sim = ScreenSim(url, *SCREEN, SIM_ID, name="Sim Screen", sw_version="9.9.9", verbose=True)
        reg = await sim.start()
        check(
            {k: reg.get(k) for k in ("screen_id", "name")} == {"screen_id": SIM_ID, "name": "Sim Screen"},
            "register returns screen_id and name",
            str(reg),
        )
        check(
            isinstance(reg.get("time_zone"), str)
            and reg["time_zone"] != ""
            and isinstance(reg.get("posix_tz"), str)
            and reg["posix_tz"] != "",
            "register returns the time zone and its POSIX rule",
            f"{reg.get('time_zone')!r} {reg.get('posix_tz')!r}",
        )
        await sim.wait_for(lambda e: e.get("kind") == "states", 5, since=0)
        kinds = [e["kind"] for e in sim.events[:2]]
        check(kinds == ["layout", "states"], "subscribe sends layout, then states", str(kinds))
        labels = {t["id"]: t["label"] for p in sim.layout["pages"] for t in p["tiles"]}
        check(all(labels.values()) and labels["t1"] == "Ceiling Lights" and labels["t10"] == "Drip line",
              "layout labels are resolved (friendly name or custom)", str(labels))
        first_states = sim.events[1]["tiles"]
        check([(p["id"], p["icon"]) for p in sim.layout["pages"]] == [("p1", "mdi:home"), ("p2", "")],
              "layout sent to the screen has page icons (accepted + defaulted to \"\")",
              str([(p["id"], p.get("icon")) for p in sim.layout["pages"]]))
        check(set(first_states) == ALL_TILES, "first states event has every tile", str(sorted(first_states)))
        sim_tiles = {t["id"]: t for p in sim.layout["pages"] for t in p["tiles"]}
        check(all("temperature_entity" in t and "humidity_entity" in t for t in sim_tiles.values())
              and sim_tiles["t14"]["temperature_entity"] == "sensor.living_room_temperature"
              and sim_tiles["t14"]["humidity_entity"] == "sensor.living_room_humidity"
              and sim_tiles["t1"]["temperature_entity"] == "",
              "layout sent to the screen has temperature_entity/humidity_entity on every tile")
        check(all(set(vm) == VM_KEYS for vm in first_states.values()), "every view model has exactly the documented keys")
        hv = first_states["t6"]
        check(hv["mode"] == "cool" and hv["target"] == 21 and hv["current"] == 22 and hv["unit"] == "°C"
              and hv["modes"][0] == "off" and hv["step"] == 0.5 and hv["min"] == 7 and hv["max"] == 35,
              "climate view model", sim.describe(hv))
        check(first_states["t2"]["brightness"] == -1 and first_states["t1"]["brightness"] > 0,
              "brightness -1 for non-dimmable light, 0-100 for dimmable",
              f"t1={first_states['t1']['brightness']} t2={first_states['t2']['brightness']}")
        check(first_states["t7"]["available"] and first_states["t7"]["status"] == "Press", "button view model", sim.describe(first_states["t7"]))
        check(hv["humidity"] == 54 and hv["detail"] == "22 °C → 21 °C · 54%",
              "climate without sensors: humidity from current_humidity, detail with humidity", f"{hv['humidity']} {hv['detail']!r}")
        room = first_states["t14"]
        check(room["current"] == 23.4 and room["humidity"] == 47 and room["detail"] == "23.4 °C → 21 °C · 47%",
              "climate with sensors: current/humidity from the sensors (humidity rounded)", sim.describe(room))
        attic = first_states["t15"]
        check(attic["current"] == 20.0 and attic["humidity"] is None and attic["detail"] == "20 °C → 20 °C",
              "climate temperature sensor in °F converted to °C; no humidity -> null", sim.describe(attic))
        check(all(first_states[t]["humidity"] is None for t in ("t1", "t3", "t5", "t11")), "humidity is null on non-climate tiles")
        auto = first_states["t11"]
        check(auto["status"] == "Enabled" and auto["on"] and (auto["detail"] == "Never ran" or auto["detail"].startswith("Ran ")),
              "automation toggle view model", sim.describe(auto))
        ib = first_states["t12"]
        check(ib["status"] == "Off" and not ib["on"] and ib["detail"] == "" and ib["brightness"] == -1 and ib["mode"] == "",
              "input_boolean toggle view model", sim.describe(ib))

        await asyncio.sleep(0.3)
        online = next(s for s in screen_lists[-1] if s["screen_id"] == SIM_ID)
        check(online["online"] and online["sw_version"] == "9.9.9" and online["model"] == "ESP32-8048S043",
              "editor sees the screen online with model/sw_version", str({k: online[k] for k in ("online", "model", "sw_version", "last_seen")}))

        devices = await admin.call("config/device_registry/list")
        dev = next((d for d in devices if ["esp32_screen", SIM_ID] in d["identifiers"]), None)
        check(dev is not None and dev["manufacturer"] == "ESP32 Screen" and dev["model"] == "ESP32-8048S043" and dev["sw_version"] == "9.9.9",
              "device registry entry", str(dev and {k: dev[k] for k in ("name", "manufacturer", "model", "sw_version")}))

        # ---- actions -----------------------------------------------------------------
        async def act(tile_id: str, action: str, value: Any, expect: Any, name: str, only_this: bool = False) -> None:
            mark = sim.mark()
            try:
                result = await sim.action(tile_id, action, value)
            except CommandError as err:
                check(False, name, str(err))
                return
            try:
                vm = await sim.wait_tile(tile_id, expect, 10, since=mark)
            except TimeoutError:
                check(False, name, f"result={result!r}, no matching state; last={sim.describe(sim.tiles[tile_id])}")
                return
            detail = sim.describe(vm)
            if only_this:
                ev = next(e for e in sim.events[mark:] if e["kind"] == "states" and tile_id in e["tiles"])
                ok = set(ev["tiles"]) == {tile_id}
                detail += f"; event tiles={sorted(ev['tiles'])}"
                check(ok and result is None, name, detail)
            else:
                check(result is None, name, detail)

        await act("t1", "brightness", 40, lambda v: v["on"] and v["brightness"] == 40 and v["detail"] == "40%", "light brightness 40 -> on 40%", only_this=True)
        await act("t1", "toggle", None, lambda v: not v["on"] and v["brightness"] == 0, "light toggle -> off")
        await act("t1", "toggle", None, lambda v: v["on"], "light toggle -> on")
        await act("t1", "brightness", 0, lambda v: not v["on"], "light brightness 0 -> off")
        await act("t2", "toggle", None, lambda v: v["on"] and v["status"] == "On" and v["brightness"] == -1, "non-dimmable light toggle -> on", only_this=True)
        await act("t9", "toggle", None, lambda v: not v["on"] and v["status"] == "Off", "switch (light kind) toggle -> off")
        await act("t3", "toggle", None, lambda v: v["on"] and v["status"] == "Open", "gate cover toggle -> open")
        await act("t3", "close", None, lambda v: not v["on"] and v["status"] == "Closed", "gate cover close")
        await act("t3", "open", None, lambda v: v["status"] == "Open", "gate cover open")
        await expect_error(sim.action("t3", "stop"), "invalid_action", "stop on a cover without stop support -> invalid_action")
        await act("t4", "toggle", None, lambda v: v["status"] == "Open", "garage toggle -> open")
        await act("t4", "toggle", None, lambda v: v["status"] == "Closed", "garage toggle -> closed")
        await act("t4", "open", None, lambda v: v["status"] == "Open", "garage open")
        await act("t4", "close", None, lambda v: v["status"] == "Closed", "garage close")

        mark = sim.mark()
        await act("t5", "toggle", None, lambda v: v["status"] == "Open" and v["on"], "valve toggle -> open")
        saw_moving = any(e["kind"] == "states" and e["tiles"].get("t5", {}).get("moving") for e in sim.events[mark:])
        check(saw_moving, "valve reports moving=true while opening")
        await act("t5", "close", None, lambda v: v["status"] == "Closed" and not v["on"], "valve close")
        await act("t5", "open", None, lambda v: v["status"] == "Open", "valve open")
        await act("t10", "open", None, lambda v: v["status"] == "Open" and v["on"], "valve (switch) open")
        await act("t10", "close", None, lambda v: v["status"] == "Closed" and not v["on"], "valve (switch) close")
        await act("t10", "toggle", None, lambda v: v["status"] == "Open", "valve (switch) toggle")

        mark = sim.mark()
        await act("t8", "close", None, lambda v: v["moving"] and v["status"] == "Closing", "cover close -> moving/closing")
        await act("t8", "toggle", None, lambda v: not v["moving"], "cover toggle while moving -> stop")
        await asyncio.sleep(1.5)
        pos_after_stop = sim.tiles["t8"]["detail"]
        await asyncio.sleep(1.5)
        check(sim.tiles["t8"]["detail"] == pos_after_stop and not sim.tiles["t8"]["moving"], "cover really stopped", pos_after_stop)

        mark_btn = sim.mark()
        try:
            res = await sim.action("t7", "toggle")
            check(res is None, "button toggle -> press succeeds")
        except CommandError as err:
            check(False, "button toggle -> press succeeds", str(err))
        await asyncio.sleep(0.5)
        check(not any(e["kind"] == "states" for e in sim.events[mark_btn:]), "button press with unchanged view model sends no states")

        await act("t6", "toggle", None, lambda v: v["mode"] == "off" and not v["on"] and v["status"] == "Off", "climate toggle -> off")
        await act("t6", "toggle", None, lambda v: v["mode"] == "cool" and v["on"], "climate toggle -> back to last mode (cool)")
        await act("t6", "set_temperature", 23.5, lambda v: v["target"] == 23.5 and "23.5 °C" in v["detail"], "climate set_temperature 23.5")
        await act("t6", "set_mode", "heat", lambda v: v["mode"] == "heat", "climate set_mode heat")
        await act("t6", "toggle", None, lambda v: v["mode"] == "off", "climate toggle -> off again")
        await act("t6", "toggle", None, lambda v: v["mode"] == "heat", "climate toggle remembers heat")
        await expect_error(sim.action("t6", "set_mode", "bogus"), "invalid_action", "climate set_mode with unknown mode -> invalid_action")
        await expect_error(sim.action("t6", "set_temperature", "hot"), "invalid_action", "climate set_temperature with text -> invalid_action")
        await expect_error(sim.action("t1", "open"), "invalid_action", "light 'open' -> invalid_action")
        await expect_error(sim.action("t1", "brightness"), "invalid_action", "brightness without value -> invalid_action")
        await expect_error(sim.action("nope", "toggle"), "not_found", "unknown tile -> not_found")
        await expect_error(sim.ws.call("esp32_screen/action", screen_id="000000000000", tile_id="t1", action="toggle"),
                           "not_found", "unknown screen -> not_found")

        # ---- toggle tiles ---------------------------------------------------------------
        await act("t11", "off", None, lambda v: v["status"] == "Disabled" and not v["on"], "automation off -> Disabled", only_this=True)
        await act("t11", "on", None, lambda v: v["status"] == "Enabled" and v["on"], "automation on -> Enabled")
        await act("t11", "toggle", None, lambda v: v["status"] == "Disabled", "automation toggle -> Disabled")
        await act("t11", "toggle", None, lambda v: v["status"] == "Enabled", "automation toggle -> Enabled")
        # (Opening the gate earlier may already have triggered it, then no new event is due.)
        await admin.call("call_service", domain="automation", service="trigger", target={"entity_id": "automation.gate_opened"}, service_data={})
        for _ in range(20):
            if sim.tiles["t16"]["detail"] == "Ran just now":
                break
            await asyncio.sleep(0.5)
        check(sim.tiles["t16"]["detail"] == "Ran just now", "automation detail shows its last run", sim.tiles["t16"]["detail"])
        await act("t12", "on", None, lambda v: v["status"] == "On" and v["on"], "input_boolean on")
        await act("t12", "off", None, lambda v: v["status"] == "Off" and not v["on"], "input_boolean off")
        await act("t12", "toggle", None, lambda v: v["status"] == "On", "input_boolean toggle -> On")
        await act("t13", "on", None, lambda v: v["status"] == "On" and v["on"], "switch (toggle kind) on")
        await act("t13", "off", None, lambda v: v["status"] == "Off", "switch (toggle kind) off")
        await act("t13", "toggle", None, lambda v: v["status"] == "On", "switch (toggle kind) toggle -> On")
        await act("t13", "toggle", None, lambda v: v["status"] == "Off", "switch (toggle kind) toggle -> Off")
        await expect_error(sim.action("t11", "open"), "invalid_action", "toggle tile 'open' -> invalid_action")
        await expect_error(sim.action("t12", "brightness", 50), "invalid_action", "toggle tile 'brightness' -> invalid_action")
        await expect_error(sim.action("t13", "stop"), "invalid_action", "toggle tile 'stop' -> invalid_action")

        # ---- climate sensors: updates and fallback ----------------------------------------
        async def set_helper(domain: str, service: str, entity: str, **data: Any) -> int:
            mark = sim.mark()
            await admin.call("call_service", domain=domain, service=service, target={"entity_id": entity}, service_data=data)
            return mark

        async def expect_tile(tile_id: str, mark: int, cond: Any, name: str, only_this: bool = False) -> None:
            try:
                vm = await sim.wait_tile(tile_id, cond, 10, since=mark)
            except TimeoutError:
                check(False, name, f"last={sim.describe(sim.tiles[tile_id])}")
                return
            detail = sim.describe(vm)
            ok = True
            if only_this:
                ev = next(e for e in sim.events[mark:] if e["kind"] == "states" and tile_id in e["tiles"])
                ok = set(ev["tiles"]) == {tile_id}
                detail += f"; event tiles={sorted(ev['tiles'])}"
            check(ok, name, detail)

        mark = await set_helper("input_number", "set_value", "input_number.room_temperature", value=24.1)
        await expect_tile("t14", mark, lambda v: v["current"] == 24.1 and v["detail"].startswith("24.1 °C"),
                          "temperature sensor change alone updates the climate tile", only_this=True)
        mark = await set_helper("input_number", "set_value", "input_number.room_humidity", value=50.4)
        await expect_tile("t14", mark, lambda v: v["humidity"] == 50 and v["detail"].endswith("· 50%"),
                          "humidity sensor change alone updates the climate tile", only_this=True)
        mark = await set_helper("input_number", "set_value", "input_number.attic_temperature_f", value=77)
        await expect_tile("t15", mark, lambda v: v["current"] == 25.0, "°F sensor 77 -> 25 °C")
        mark = await set_helper("input_boolean", "turn_off", "input_boolean.room_sensor_online")
        await expect_tile("t14", mark, lambda v: v["current"] == 22 and v["humidity"] == 54,
                          "unavailable sensors fall back to the AC's own readings")
        mark = await set_helper("input_boolean", "turn_on", "input_boolean.room_sensor_online")
        await expect_tile("t14", mark, lambda v: v["current"] == 24.1 and v["humidity"] == 50,
                          "sensors back -> their values again")
        await set_helper("input_number", "set_value", "input_number.room_temperature", value=23.4)
        await set_helper("input_number", "set_value", "input_number.room_humidity", value=46.6)
        await set_helper("input_number", "set_value", "input_number.attic_temperature_f", value=68)
        await set_helper("input_boolean", "turn_off", "input_boolean.guest_mode")

        # Restore some demo defaults so the editor preview looks lively.
        for tid, action, value in (("t1", "brightness", 80), ("t6", "set_mode", "cool"), ("t9", "toggle", None)):
            await sim.action(tid, action, value)

        # ---- non-admin is rejected by editor commands ---------------------------------
        await expect_error(sim.ws.call("esp32_screen/screens"), "unauthorized", "non-admin: screens rejected")
        await expect_error(sim.ws.call("esp32_screen/layout/save", screen_id=SIM_ID, layout=TEST_LAYOUT), "unauthorized", "non-admin: layout/save rejected")
        await expect_error(sim.ws.call("esp32_screen/screen/update", screen_id=SIM_ID, name="x"), "unauthorized", "non-admin: screen/update rejected")
        await expect_error(sim.ws.call("esp32_screen/screen/remove", screen_id=SIM_ID), "unauthorized", "non-admin: screen/remove rejected")
        await expect_error(sim.ws.subscribe(lambda e: None, "esp32_screen/subscribe_screens"), "unauthorized", "non-admin: subscribe_screens rejected")

        # ---- saving pushes layout + all states to the screen ------------------------------
        mark = sim.mark()
        changed = copy.deepcopy(TEST_LAYOUT)
        changed["pages"][0]["tiles"][0]["label"] = "Ceiling"
        await admin.call("esp32_screen/layout/save", screen_id=SIM_ID, layout=changed)
        ev = await sim.wait_for(lambda e: e["kind"] == "states", 5, since=mark)
        kinds = [e["kind"] for e in sim.events[mark:]]
        check(kinds[:2] == ["layout", "states"] and len(ev["tiles"]) == len(ALL_TILES) and sim.layout["pages"][0]["tiles"][0]["label"] == "Ceiling",
              "saving the layout re-sends layout + all states", str(kinds))

        # ---- a friendly-name change re-sends the layout with the new label -----------------
        mark = sim.mark()
        await admin.call("config/entity_registry/update", entity_id="light.porch_light", name="Porch")
        try:
            await sim.wait_for(lambda e: e["kind"] == "layout", 5, since=mark)
            label = next(t["label"] for t in sim.layout["pages"][0]["tiles"] if t["id"] == "t2")
            check(label == "Porch", "renaming an entity re-sends the layout with the new label", label)
        except TimeoutError:
            check(False, "renaming an entity re-sends the layout with the new label", "no layout event")
        await admin.call("config/entity_registry/update", entity_id="light.porch_light", name=None)

        # ---- rename wins over the name sent by register -----------------------------------
        await admin.call("esp32_screen/screen/update", screen_id=SIM_ID, name="Hallway")
        reg2 = await sim.ws.call("esp32_screen/register", screen_id=SIM_ID, name="Sim Screen", model="ESP32-8048S043", sw_version="9.9.9")
        check(reg2["name"] == "Hallway", "name set in HA wins over the name sent by register", str(reg2))

        # ---- unknown screen subscription --------------------------------------------------
        unknown_events: list[dict[str, Any]] = []
        await sim.ws.subscribe(unknown_events.append, "esp32_screen/subscribe", screen_id="FFFFFFFFFFFF")
        await asyncio.sleep(0.3)
        check(unknown_events[:2] == [{"kind": "layout", "layout": {"version": 1, "pages": []}}, {"kind": "states", "tiles": {}}],
              "unknown screen gets an empty layout", str(unknown_events[:2]))

        # ---- firmware updates (section 5) -------------------------------------------------------
        await firmware_tests(admin, sim, firmware_dir, session, url)

        # ---- token refresh (protocol 1.4) -----------------------------------------------------
        assert sim.session is not None
        refreshed = await refresh_token(sim.session, url, sim.client_id, sim.tokens["refresh_token"])
        check("access_token" in refreshed and refreshed.get("expires_in") == 1800, "refresh token grant works")

        # ---- disconnect -> offline ----------------------------------------------------------
        await sim.stop()
        await asyncio.sleep(0.5)
        rec = next(s for s in screen_lists[-1] if s["screen_id"] == SIM_ID)
        check(not rec["online"] and rec["last_seen"], "screen goes offline on disconnect, last_seen kept", str({k: rec[k] for k in ("online", "last_seen")}))

        # ---- remove ----------------------------------------------------------------------------
        await admin.call("esp32_screen/layout/save", screen_id=TEMP_ID, layout={"version": 1, "pages": []})
        devices = await admin.call("config/device_registry/list")
        had_device = any(["esp32_screen", TEMP_ID] in d["identifiers"] for d in devices)
        await admin.call("esp32_screen/screen/remove", screen_id=TEMP_ID)
        screens = (await admin.call("esp32_screen/screens"))["screens"]
        devices = await admin.call("config/device_registry/list")
        gone = not any(s["screen_id"] == TEMP_ID for s in screens) and not any(["esp32_screen", TEMP_ID] in d["identifiers"] for d in devices)
        check(had_device and gone, "screen/remove deletes record and device")
        await expect_error(admin.call("esp32_screen/screen/remove", screen_id=TEMP_ID), "not_found", "screen/remove unknown -> not_found")

        # Leave the sim screen with its original name for the editor demo.
        await admin.call("esp32_screen/screen/update", screen_id=SIM_ID, name="Simulated Screen")
        await admin.close()

    failed = [r for r in RESULTS if not r[0]]
    print(f"\n{len(RESULTS) - len(failed)}/{len(RESULTS)} checks passed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
