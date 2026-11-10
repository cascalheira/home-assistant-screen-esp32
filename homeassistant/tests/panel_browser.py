"""Drive the Screens panel in headless Chromium and take screenshots.

    uv run --with aiohttp --with playwright homeassistant/tests/panel_browser.py [--url ...]

(First time: `uvx playwright install chromium`.)

Logs in as the admin `dev` through HA's login form, opens
/esp32-screen, selects the simulated screen AABBCCDDEEFF, adds a page, adds a tile
by clicking a cell, drags it, resizes it, saves, and checks the saved layout
through the websocket API. Also provokes a backend validation error and takes
light/dark/narrow screenshots of the real screen's layout. Screenshots go to
/tmp/ha-editor-*.png.
"""

from __future__ import annotations

import argparse
import asyncio
import sys
from pathlib import Path
from typing import Any

import aiohttp
from playwright.async_api import Page, async_playwright

sys.path.insert(0, str(Path(__file__).parent))
from device_sim import HAWebSocket, login_flow  # noqa: E402

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "dev"))
import local  # noqa: E402

SIM_ID = "AABBCCDDEEFF"
REAL_ID = local.real_screen_id()  # "" when no real screen is registered
USER = local.admin()
RESULTS: list[tuple[bool, str, str]] = []


def check(ok: bool, name: str, detail: str = "") -> None:
    RESULTS.append((ok, name, detail))
    print(f"{'PASS' if ok else 'FAIL'}  {name}" + (f"  ({detail})" if detail else ""), flush=True)


async def open_panel(browser: Any, url: str, *, dark: bool, width: int, height: int) -> Page:
    context = await browser.new_context(
        viewport={"width": width, "height": height},
        color_scheme="dark" if dark else "light",
        device_scale_factor=1,
        # HA's service worker installs on the first visit and then reloads the
        # page at a random moment, which recreates the panel mid-test.
        service_workers="block",
    )
    page = await context.new_page()
    errors: list[str] = []
    # Only errors from our panel count (blocking the service worker makes HA's
    # own code throw on navigator.serviceWorker).
    page.on("pageerror", lambda exc: errors.append(str(exc)) if "esp32-screen-panel" in (exc.stack or "") else None)
    page.on("console", lambda msg: errors.append(msg.text) if msg.type == "error" and "esp32" in msg.text.lower() else None)
    page.errors = errors  # type: ignore[attr-defined]
    await page.goto(f"{url}/esp32-screen")
    # The current frontend uses PKCE and ignores tokens planted in localStorage
    # `hassTokens`, so log in through the real login form.
    await page.wait_for_url("**/auth/authorize**", timeout=15000)
    await page.locator("input[name='username']").fill(USER[0])
    await page.locator("input[name='password']").fill(USER[1])
    await page.keyboard.press("Enter")
    await page.wait_for_url(f"{url}/esp32-screen", timeout=15000)
    await page.locator("esp32-screen-panel .screen-item").first.wait_for(timeout=30000)
    return page


async def select_screen(page: Page, screen_id: str) -> None:
    await page.locator(f"esp32-screen-panel .screen-item[data-id='{screen_id}']").click()
    await page.locator("esp32-screen-panel .device, esp32-screen-panel .empty").first.wait_for()


async def cell_center(page: Page, x: int, y: int, cols: int, rows: int) -> tuple[float, float]:
    box = await page.locator("esp32-screen-panel #grid").bounding_box()
    assert box
    return box["x"] + (x + 0.5) * box["width"] / cols, box["y"] + (y + 0.5) * box["height"] / rows


async def drag(page: Page, start: tuple[float, float], end: tuple[float, float]) -> None:
    await page.mouse.move(*start)
    await page.mouse.down()
    await page.mouse.move(start[0] + 8, start[1] + 8, steps=3)
    await page.mouse.move(*end, steps=12)
    await page.mouse.up()


async def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--url", default="http://localhost:8123")
    parser.add_argument("--headed", action="store_true")
    url = parser.parse_args().url.rstrip("/")

    async with aiohttp.ClientSession() as session:
        tokens = await login_flow(session, url, f"{url}/", *USER)
        ws = HAWebSocket(session, url)
        await ws.connect(tokens["access_token"])

        async def saved_layout(screen_id: str) -> dict[str, Any]:
            screens = (await ws.call("esp32_screen/screens"))["screens"]
            return next(s for s in screens if s["screen_id"] == screen_id)["layout"]

        before = await saved_layout(SIM_ID)
        if any(p["title"] == "Browser test" for p in before["pages"]):
            # Left over from an interrupted run.
            before["pages"] = [p for p in before["pages"] if p["title"] != "Browser test"]
            await ws.call("esp32_screen/layout/save", screen_id=SIM_ID, layout=before)
        n_pages = len(before["pages"])

        async with async_playwright() as pw:
            browser = await pw.chromium.launch(headless=True)
            page = await open_panel(browser, url, dark=False, width=1600, height=1000)
            await select_screen(page, SIM_ID)
            panel = page.locator("esp32-screen-panel")
            await page.screenshot(path="/tmp/ha-editor-1-loaded.png")
            check(await panel.locator(".tile").count() > 0, "panel loads and shows the simulated screen's tiles")

            # Add a page.
            await panel.locator(".tab.add").click()
            check(await panel.locator(".tab").count() == n_pages + 2, "add page")
            title = panel.locator("[data-input='page-title']")
            await title.fill("Browser test")
            await panel.locator("[data-change='page-cols']").select_option("4")
            await panel.locator("[data-change='page-rows']").select_option("3")
            check(await panel.locator(".unsaved").count() == 1, "unsaved-changes indicator shows")

            # Page icon picker.
            await panel.locator("[data-action='icon-picker']").click()
            choices = await panel.locator(".icon-choice").evaluate_all("els => els.map(e => e.dataset.icon)")
            check(len(choices) == 47 and choices[0] == "" and choices[1] == "mdi:home" and choices[-1] == "mdi:cctv",
                  "icon picker lists 'No icon' + the 46 allowed icons in order", f"{len(choices)} choices")
            # ha-icon loads MDI paths lazily; wait until every choice has drawn its glyph.
            drawn = 0
            try:
                await page.wait_for_function(
                    """() => {
                      const panel = document.querySelector("home-assistant").shadowRoot
                        .querySelector("home-assistant-main").shadowRoot.querySelector("esp32-screen-panel");
                      const icons = [...panel.shadowRoot.querySelectorAll(".icon-choice ha-icon")];
                      const ok = icons.filter((i) => i.shadowRoot?.querySelector("ha-svg-icon")?.path);
                      return icons.length === 46 && ok.length === 46;
                    }""",
                    timeout=15000,
                )
                drawn = 46
            except Exception:  # noqa: BLE001
                drawn = -1
            check(drawn == 46, "all 46 page icons exist in HA's icon set and render in the picker")
            await page.screenshot(path="/tmp/ha-editor-13-icon-picker.png")
            await panel.locator(".icon-choice[data-icon='mdi:bed']").click()
            check(await panel.locator(".icon-popover").count() == 0
                  and await panel.locator(".tab.active ha-icon[icon='mdi:bed']").count() == 1
                  and await panel.locator(".dtab.active ha-icon[icon='mdi:bed']").count() == 1,
                  "picked icon shows on the editor tab and the preview tab")
            grid_box = await panel.locator("#grid").bounding_box()
            assert grid_box
            ratio = grid_box["width"] / grid_box["height"]
            check(abs(ratio - 800 / 412) < 0.01, "preview grid has the 800x412 aspect ratio", f"{ratio:.3f} vs {800 / 412:.3f}")

            # Click an empty cell -> add a light tile.
            await panel.locator(".cell[data-x='0'][data-y='0']").click()
            await panel.locator(".kind[data-kind='light']").click()
            await panel.locator("[data-input='entity-filter']").fill("bed")
            await panel.locator(".entity[data-entity='light.bed_light']").click()
            await panel.locator("[data-input='tile-label']").fill("Bed")
            await page.screenshot(path="/tmp/ha-editor-2-add-tile.png")
            await panel.locator("[data-action='confirm-new']").click()
            check(await panel.locator(".tile").count() == 1, "tile added by clicking a cell")

            # Add a climate tile (defaults to 2x1) at (0,2).
            await panel.locator(".cell[data-x='0'][data-y='2']").click()
            await panel.locator(".kind[data-kind='climate']").click()
            await panel.locator(".entity[data-entity='climate.hvac']").click()
            await panel.locator("[data-action='confirm-new']").click()

            # Drag the light from (0,0) to (2,1).
            await drag(page, await cell_center(page, 0, 0, 4, 3), await cell_center(page, 2, 1, 4, 3))
            # Try to drag it onto the climate tile (must be refused).
            await drag(page, await cell_center(page, 2, 1, 4, 3), await cell_center(page, 1, 2, 4, 3))
            # Resize it with the corner handle to 2x2... which would overlap nothing: (2..3, 1..2).
            handle = panel.locator(".tile[data-tile-id] .handle").first
            light_tile = panel.locator(".tile", has_text="Bed")
            hbox = await light_tile.locator(".handle").bounding_box()
            assert hbox
            await drag(page, (hbox["x"] + hbox["width"] / 2, hbox["y"] + hbox["height"] / 2), await cell_center(page, 3, 2, 4, 3))
            del handle
            await page.screenshot(path="/tmp/ha-editor-3-moved-resized.png")

            await panel.locator("[data-action='save']").click()
            await panel.locator(".unsaved").wait_for(state="detached", timeout=5000)
            await page.screenshot(path="/tmp/ha-editor-4-saved.png")
            after = await saved_layout(SIM_ID)
            new_page = next((p for p in after["pages"] if p["title"] == "Browser test"), None)
            tiles = {t["entity_id"]: t for t in (new_page or {}).get("tiles", [])}
            bed = tiles.get("light.bed_light")
            hvac = tiles.get("climate.hvac")
            check(new_page is not None and new_page["cols"] == 4 and new_page["rows"] == 3 and new_page["icon"] == "mdi:bed",
                  "saved page with 4x3 grid and icon mdi:bed", str(new_page and new_page["icon"]))
            check(bed is not None and (bed["x"], bed["y"], bed["w"], bed["h"]) == (2, 1, 2, 2) and bed["label"] == "Bed",
                  "tile was moved to (2,1), resized to 2x2 and saved", str(bed))
            check(hvac is not None and (hvac["x"], hvac["y"], hvac["w"], hvac["h"]) == (0, 2, 2, 1),
                  "climate tile 2x1 at (0,2); overlapping drag was refused", str(hvac))

            # Climate tile: pick external temperature and humidity sensors.
            await panel.locator(".tile", has_text="Hvac").click()
            temp_select = panel.locator("select[data-change='temperature_entity']")
            hum_select = panel.locator("select[data-change='humidity_entity']")
            first_opts = await temp_select.locator("option").all_inner_texts()
            matching = await temp_select.locator("optgroup[label='Matching sensors'] option").all_inner_texts()
            check(first_opts[0] == "Use the AC's own reading" and any("Living Room Temperature (23.4 °C)" in o for o in matching),
                  "temperature picker: AC option first, matching sensors with values", "; ".join(matching[:4]))
            await temp_select.select_option("sensor.living_room_temperature")
            await hum_select.select_option("sensor.living_room_humidity")
            hvac_text = await panel.locator(".tile", has_text="Hvac").inner_text()
            check("23.4 °C" in hvac_text and "47%" in hvac_text, "tile preview uses the sensors (temperature + humidity)", " ".join(hvac_text.split()))
            await page.screenshot(path="/tmp/ha-editor-9-climate-sensors.png")

            # Toggle tile.
            await panel.locator(".cell[data-x='0'][data-y='0']").click()
            await panel.locator(".kind[data-kind='toggle']").click()
            entity_ids = await panel.locator(".entity").evaluate_all("els => els.map(e => e.dataset.entity)")
            check(entity_ids and all(e.split(".")[0] in ("automation", "input_boolean", "switch") for e in entity_ids)
                  and "automation.night_lights" in entity_ids,
                  "toggle kind lists only automation/input_boolean/switch", f"{len(entity_ids)} entities")
            await panel.locator(".entity[data-entity='automation.night_lights']").click()
            await panel.locator("[data-action='confirm-new']").click()
            await panel.locator(".tile", has_text="Night lights").click()
            toggle_text = await panel.locator(".tile", has_text="Night lights").inner_text()
            # The live screen may toggle this automation meanwhile, so accept either state.
            check(("Enabled" in toggle_text or "Disabled" in toggle_text) and "ran" in toggle_text.lower(),
                  "toggle tile preview shows Enabled/Disabled + last run", " ".join(toggle_text.split()))
            await page.screenshot(path="/tmp/ha-editor-10-toggle.png")
            await panel.locator("[data-action='save']").click()
            await panel.locator(".unsaved").wait_for(state="detached", timeout=5000)
            after = await saved_layout(SIM_ID)
            new_page = next((p for p in after["pages"] if p["title"] == "Browser test"), {})
            tiles = {t["entity_id"]: t for t in new_page.get("tiles", [])}
            hvac = tiles.get("climate.hvac", {})
            night = tiles.get("automation.night_lights", {})
            check(hvac.get("temperature_entity") == "sensor.living_room_temperature"
                  and hvac.get("humidity_entity") == "sensor.living_room_humidity"
                  and night.get("kind") == "toggle" and night.get("temperature_entity") == "",
                  "sensors and toggle tile saved", f"hvac={hvac.get('temperature_entity')},{hvac.get('humidity_entity')} night={night.get('kind')}")

            # Backend validation error: switching kind clears an incompatible entity.
            await light_tile.click()
            await panel.locator(".kind[data-kind='valve']").click()
            await panel.locator("[data-action='save']").click()
            err = panel.locator(".error-box")
            await err.wait_for(timeout=5000)
            text = await err.inner_text()
            check("pick an entity" in text, "backend validation error is shown", text.strip())
            await page.screenshot(path="/tmp/ha-editor-5-validation-error.png")
            page.once("dialog", lambda d: asyncio.ensure_future(d.accept()))
            await panel.locator("[data-action='discard']").click()
            await panel.locator(".unsaved").wait_for(state="detached", timeout=5000)

            # Remove the test page again so the sim layout stays as e2e.py left it.
            await panel.locator(".tab", has_text="Browser test").click()
            page.once("dialog", lambda d: asyncio.ensure_future(d.accept()))
            await panel.locator("[data-action='delete-page']").click()
            await panel.locator("[data-action='save']").click()
            await panel.locator(".unsaved").wait_for(state="detached", timeout=5000)
            check(len((await saved_layout(SIM_ID))["pages"]) == n_pages, "page deleted and saved")
            light_errors = list(page.errors)  # type: ignore[attr-defined]
            await page.context.close()

            # Real screen (if one is configured), light + dark theme, wide + narrow.
            for name, dark, width, height in (
                ("6-real-light", False, 1600, 1000),
                ("7-real-dark", True, 1600, 1000),
                ("8-narrow-dark", True, 420, 900),
            ) if REAL_ID else ():
                p = await open_panel(browser, url, dark=dark, width=width, height=height)
                await select_screen(p, REAL_ID)
                await p.wait_for_timeout(1000)
                await p.screenshot(path=f"/tmp/ha-editor-{name}.png", full_page=True)
                if name == "7-real-dark":
                    await p.locator("esp32-screen-panel .tab", has_text="Climate").click()
                    await p.wait_for_timeout(500)
                    await p.screenshot(path="/tmp/ha-editor-7b-real-dark-climate.png")
                if name == "6-real-light":
                    await p.locator("esp32-screen-panel .device").screenshot(path="/tmp/ha-editor-13-preview-icons.png")
                    tab_icons = await p.locator("esp32-screen-panel .dtab ha-icon").evaluate_all("els => els.map(e => e.getAttribute('icon'))")
                    check(tab_icons == ["mdi:sofa", "mdi:thermostat", "mdi:robot"], "real screen preview tabs show page icons", str(tab_icons))
                    await p.locator("esp32-screen-panel .tile", has_text="Hvac").click()
                    await p.wait_for_timeout(300)
                    await p.screenshot(path="/tmp/ha-editor-11-real-climate-sensors.png")
                    await p.locator("esp32-screen-panel .tab", has_text="Automations").click()
                    await p.locator("esp32-screen-panel .tile", has_text="Guest mode").click()
                    await p.wait_for_timeout(300)
                    await p.screenshot(path="/tmp/ha-editor-12-real-automations.png")
                    check(await p.locator("esp32-screen-panel .unsaved").count() == 0, "selecting tiles does not create unsaved changes")
                if name == "8-narrow-dark":
                    dev = await p.locator("esp32-screen-panel .device").bounding_box()
                    check(dev is not None and dev["width"] <= width, "narrow layout: device preview fits the viewport", str(dev))
                light_errors += list(p.errors)  # type: ignore[attr-defined]
                await p.context.close()
            check(not light_errors, "no page errors from the panel", "; ".join(light_errors)[:300])
            await browser.close()
        await ws.close()

    failed = [r for r in RESULTS if not r[0]]
    print(f"\n{len(RESULTS) - len(failed)}/{len(RESULTS)} checks passed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
