"""Onboard the development Home Assistant without a browser.

    uv run --with aiohttp homeassistant/dev/onboard.py [--url http://localhost:8123]

Idempotent. Creates the owner (admin) and a non-admin user for screens, finishes
onboarding and adds the esp32_screen integration through its config flow. User
names and (random) passwords come from the git-ignored
`dev-config/dev-local.json`, see `local.py`.
"""

from __future__ import annotations

import argparse
import asyncio
import itertools
from typing import Any

import sys
from pathlib import Path

import aiohttp

sys.path.insert(0, str(Path(__file__).resolve().parent))
import local  # noqa: E402

OWNER = (*local.admin(), "Dev")
SCREEN_USER = (*local.screen_user(), "Screen")


async def token_from_code(session: aiohttp.ClientSession, url: str, code: str, cid: str) -> str:
    async with session.post(
        f"{url}/auth/token",
        data={"grant_type": "authorization_code", "code": code, "client_id": cid},
    ) as resp:
        resp.raise_for_status()
        return (await resp.json())["access_token"]


async def login(session: aiohttp.ClientSession, url: str, username: str, password: str) -> str:
    cid = f"{url}/"
    async with session.post(
        f"{url}/auth/login_flow",
        json={"client_id": cid, "handler": ["homeassistant", None], "redirect_uri": f"{cid}?auth_callback=1"},
    ) as resp:
        flow = await resp.json()
    async with session.post(
        f"{url}/auth/login_flow/{flow['flow_id']}",
        json={"client_id": cid, "username": username, "password": password},
    ) as resp:
        result = await resp.json()
    if result.get("type") != "create_entry":
        raise SystemExit(f"login as {username} failed: {result}")
    return await token_from_code(session, url, result["result"], cid)


class WS:
    def __init__(self, ws: aiohttp.ClientWebSocketResponse) -> None:
        self.ws = ws
        self.ids = itertools.count(1)

    async def call(self, type_: str, **payload: Any) -> Any:
        msg_id = next(self.ids)
        await self.ws.send_json({"id": msg_id, "type": type_, **payload})
        while True:
            msg = await self.ws.receive_json()
            if msg.get("id") == msg_id and msg["type"] == "result":
                if not msg["success"]:
                    raise RuntimeError(f"{type_}: {msg['error']}")
                return msg["result"]


async def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--url", default="http://localhost:8123")
    url = parser.parse_args().url.rstrip("/")
    cid = f"{url}/"

    async with aiohttp.ClientSession() as session:
        for _ in range(60):
            try:
                async with session.get(f"{url}/api/onboarding") as resp:
                    if resp.status == 200:
                        steps = {s["step"]: s["done"] for s in await resp.json()}
                        break
            except aiohttp.ClientError:
                pass
            await asyncio.sleep(2)
        else:
            raise SystemExit("Home Assistant is not reachable")

        if not steps.get("user"):
            async with session.post(
                f"{url}/api/onboarding/users",
                json={"client_id": cid, "name": OWNER[2], "username": OWNER[0], "password": OWNER[1], "language": "en"},
            ) as resp:
                resp.raise_for_status()
                code = (await resp.json())["auth_code"]
            token = await token_from_code(session, url, code, cid)
            print("created owner user 'dev'")
        else:
            token = await login(session, url, OWNER[0], OWNER[1])
            print("owner user already exists")
        auth = {"Authorization": f"Bearer {token}"}

        for step, path, body in (
            ("core_config", "core_config", {}),
            ("analytics", "analytics", {}),
            ("integration", "integration", {"client_id": cid, "redirect_uri": f"{cid}?auth_callback=1"}),
        ):
            if not steps.get(step):
                async with session.post(f"{url}/api/onboarding/{path}", json=body, headers=auth) as resp:
                    print(f"onboarding step {step}: HTTP {resp.status}")

        async with session.ws_connect(f"{url}/api/websocket") as raw:
            await raw.receive_json()
            await raw.send_json({"type": "auth", "access_token": token})
            assert (await raw.receive_json())["type"] == "auth_ok"
            ws = WS(raw)
            users = await ws.call("config/auth/list")
            existing = next(
                (u for u in users if any(c.get("data", {}).get("username") == SCREEN_USER[0] for c in u.get("credentials", []))),
                None,
            ) or next((u for u in users if u.get("username") == SCREEN_USER[0]), None)
            if existing is None:
                user = (await ws.call("config/auth/create", name=SCREEN_USER[2], group_ids=["system-users"], local_only=False))["user"]
                await ws.call(
                    "config/auth_provider/homeassistant/create",
                    user_id=user["id"], username=SCREEN_USER[0], password=SCREEN_USER[1],
                )
                print(f"created non-admin user 'screen' ({user['id']})")
            else:
                print(f"user 'screen' already exists (admin={existing.get('is_admin')})")

        async with session.get(f"{url}/api/config/config_entries/entry", headers=auth) as resp:
            entries = await resp.json()
        if any(e["domain"] == "esp32_screen" for e in entries):
            print("esp32_screen integration already added")
            return
        async with session.post(
            f"{url}/api/config/config_entries/flow", json={"handler": "esp32_screen"}, headers=auth
        ) as resp:
            flow = await resp.json()
        if flow.get("type") == "form":
            async with session.post(
                f"{url}/api/config/config_entries/flow/{flow['flow_id']}", json={}, headers=auth
            ) as resp:
                flow = await resp.json()
        print(f"config flow result: {flow.get('type')} {flow.get('title', flow.get('reason', ''))}")


if __name__ == "__main__":
    asyncio.run(main())
