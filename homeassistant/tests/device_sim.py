"""Simulated ESP32 screen: speaks docs/protocol.md exactly like the firmware should.

    uv run --with aiohttp homeassistant/tests/device_sim.py \
        --url http://localhost:8123 [--user USER --password PASSWORD] \
        --screen-id AABBCCDDEEFF [--action t1:toggle --action t1:brightness:40] \
        [--duration 10 | --interactive]

Logs in with HA's login flow (protocol section 1), opens the websocket, sends
`esp32_screen/register`, `esp32_screen/subscribe` and prints every event. Actions
are `TILE:ACTION[:VALUE]`. In --interactive mode, type `t1 toggle`,
`t1 brightness 40`, `t6 set_mode heat` or `quit`.

With --ota apply it performs firmware updates like the firmware: download,
check size and sha256, report progress via `esp32_screen/ota_status`, then
"restart" (reconnect and register with the new version). --ota fail reports a
failure instead.

Also importable: tests/e2e.py drives `ScreenSim` directly.
"""

from __future__ import annotations

import argparse
import asyncio
import itertools
import json
import sys
from pathlib import Path
from typing import Any

import aiohttp


class CommandError(Exception):
    """A websocket command returned success=false."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(f"{code}: {message}")
        self.code = code
        self.message = message


class LoginError(Exception):
    """The login flow failed."""


async def login_flow(
    session: aiohttp.ClientSession, url: str, client_id: str, username: str, password: str
) -> dict[str, Any]:
    """Protocol section 1, steps 1-3. Returns the token response."""
    redirect_uri = client_id + "auth_callback"
    # 1. start the flow
    async with session.post(
        f"{url}/auth/login_flow",
        json={"client_id": client_id, "handler": ["homeassistant", None], "redirect_uri": redirect_uri},
    ) as resp:
        flow = await resp.json()
    if resp.status != 200 or flow.get("type") != "form":
        raise LoginError(f"login_flow start failed: HTTP {resp.status} {flow}")
    # 2. send credentials
    async with session.post(
        f"{url}/auth/login_flow/{flow['flow_id']}",
        json={"client_id": client_id, "username": username, "password": password},
    ) as resp:
        step = await resp.json()
    if step.get("type") == "form" and step.get("step_id") == "mfa":
        raise LoginError("MFA is enabled for this user; the simulator does not support it")
    if step.get("type") != "create_entry":
        raise LoginError(f"login rejected: {step.get('errors') or step}")
    # 3. exchange the code for tokens (form-encoded)
    async with session.post(
        f"{url}/auth/token",
        data={"grant_type": "authorization_code", "code": step["result"], "client_id": client_id},
    ) as resp:
        tokens = await resp.json()
    if resp.status != 200 or "access_token" not in tokens:
        raise LoginError(f"token exchange failed: HTTP {resp.status} {tokens}")
    return tokens


async def refresh_token(
    session: aiohttp.ClientSession, url: str, client_id: str, refresh: str
) -> dict[str, Any]:
    """Protocol section 1, step 4."""
    async with session.post(
        f"{url}/auth/token",
        data={"grant_type": "refresh_token", "refresh_token": refresh, "client_id": client_id},
    ) as resp:
        tokens = await resp.json()
    if resp.status != 200:
        raise LoginError(f"refresh failed: HTTP {resp.status} {tokens}")
    return tokens


class HAWebSocket:
    """Minimal HA websocket client with results, subscriptions and an event log."""

    def __init__(self, session: aiohttp.ClientSession, url: str, *, verbose: bool = False, tag: str = "") -> None:
        self.session = session
        self.url = url
        self.verbose = verbose
        self.tag = tag
        self.ws: aiohttp.ClientWebSocketResponse | None = None
        self._ids = itertools.count(1)
        self._pending: dict[int, asyncio.Future[Any]] = {}
        self._handlers: dict[int, Any] = {}
        self._reader: asyncio.Task[None] | None = None

    async def connect(self, access_token: str) -> str:
        ws_url = self.url.replace("http", "ws", 1) + "/api/websocket"
        self.ws = await self.session.ws_connect(ws_url, heartbeat=30)
        hello = await self.ws.receive_json()
        assert hello["type"] == "auth_required", hello
        await self.ws.send_json({"type": "auth", "access_token": access_token})
        auth = await self.ws.receive_json()
        if auth["type"] != "auth_ok":
            raise LoginError(f"websocket auth failed: {auth}")
        self._reader = asyncio.create_task(self._read())
        return str(auth.get("ha_version"))

    async def close(self) -> None:
        if self.ws is not None:
            await self.ws.close()
        if self._reader is not None:
            self._reader.cancel()

    async def _read(self) -> None:
        assert self.ws is not None
        async for raw in self.ws:
            if raw.type != aiohttp.WSMsgType.TEXT:
                continue
            data = json.loads(raw.data)
            for msg in data if isinstance(data, list) else [data]:
                if msg.get("type") == "result" and (fut := self._pending.pop(msg["id"], None)):
                    if msg["success"]:
                        fut.set_result(msg["result"])
                    else:
                        err = msg.get("error", {})
                        fut.set_exception(CommandError(err.get("code", "?"), err.get("message", "")))
                elif msg.get("type") == "event" and (handler := self._handlers.get(msg["id"])):
                    handler(msg["event"])
        for fut in self._pending.values():
            fut.set_exception(ConnectionError("websocket closed"))

    async def call(self, type_: str, **payload: Any) -> Any:
        assert self.ws is not None
        msg_id = next(self._ids)
        fut: asyncio.Future[Any] = asyncio.get_running_loop().create_future()
        self._pending[msg_id] = fut
        await self.ws.send_json({"id": msg_id, "type": type_, **payload})
        return await asyncio.wait_for(fut, 15)

    async def subscribe(self, handler: Any, type_: str, **payload: Any) -> int:
        assert self.ws is not None
        msg_id = next(self._ids)
        self._handlers[msg_id] = handler
        fut: asyncio.Future[Any] = asyncio.get_running_loop().create_future()
        self._pending[msg_id] = fut
        await self.ws.send_json({"id": msg_id, "type": type_, **payload})
        await asyncio.wait_for(fut, 15)
        return msg_id


VM_KEYS = {
    "available", "on", "status", "detail", "moving", "brightness", "current",
    "target", "min", "max", "step", "mode", "modes", "unit", "humidity",
}


class ScreenSim:
    """One simulated screen."""

    def __init__(
        self,
        url: str,
        username: str,
        password: str,
        screen_id: str,
        *,
        name: str = "Simulated Screen",
        model: str = "ESP32-8048S043",
        sw_version: str = "0.0.0-sim",
        verbose: bool = True,
    ) -> None:
        self.url = url.rstrip("/")
        self.username = username
        self.password = password
        self.screen_id = screen_id
        self.name = name
        self.model = model
        self.sw_version = sw_version
        self.verbose = verbose
        self.client_id = f"http://esp32-screen-{screen_id}.local/"
        self.session: aiohttp.ClientSession | None = None
        self.ws: HAWebSocket | None = None
        self.tokens: dict[str, Any] = {}
        self.layout: dict[str, Any] | None = None
        self.tiles: dict[str, dict[str, Any]] = {}
        self.events: list[dict[str, Any]] = []
        self._event_cond = asyncio.Condition()
        self.ota_mode = "ignore"  # ignore | apply | fail (CLI --ota)

    def log(self, *args: Any) -> None:
        if self.verbose:
            print(f"[{self.screen_id}]", *args, flush=True)

    async def start(self) -> dict[str, Any]:
        """Log in, connect, register, subscribe. Returns the register result."""
        self.session = aiohttp.ClientSession()
        self.tokens = await login_flow(self.session, self.url, self.client_id, self.username, self.password)
        self.log(f"logged in as {self.username}; token expires_in={self.tokens.get('expires_in')}")
        self.ws = HAWebSocket(self.session, self.url)
        version = await self.ws.connect(self.tokens["access_token"])
        self.log(f"websocket auth_ok (HA {version})")
        reg = await self.ws.call(
            "esp32_screen/register",
            screen_id=self.screen_id, name=self.name, model=self.model, sw_version=self.sw_version,
        )
        self.log("registered:", reg)
        await self.ws.subscribe(self._on_event, "esp32_screen/subscribe", screen_id=self.screen_id)
        self.log("subscribed")
        return reg

    async def stop(self) -> None:
        if self.ws is not None:
            await self.ws.close()
        if self.session is not None:
            await self.session.close()

    def _on_event(self, event: dict[str, Any]) -> None:
        self.events.append(event)
        if event.get("kind") == "layout":
            self.layout = event["layout"]
            pages = self.layout.get("pages", [])
            self.log(
                "layout:",
                ", ".join(f"{p['title']} {p['cols']}x{p['rows']} ({len(p['tiles'])} tiles)" for p in pages) or "(empty)",
            )
            for page in pages:
                for t in page["tiles"]:
                    sensors = " ".join(f"{k.split('_')[0]}={t.get(k)}" for k in ("temperature_entity", "humidity_entity") if t.get(k))
                    self.log(f"   {t['id']:<4} {t['kind']:<8} {t['entity_id']:<32} '{t['label']}' @{t['x']},{t['y']} {t['w']}x{t['h']} {sensors}")
        elif event.get("kind") == "states":
            for tile_id, vm in event["tiles"].items():
                self.tiles[tile_id] = vm
                missing = VM_KEYS - vm.keys()
                extra = vm.keys() - VM_KEYS
                note = f" MISSING KEYS {missing}" if missing else ""
                note += f" EXTRA KEYS {extra}" if extra else ""
                self.log(f"states {tile_id}: {self.describe(vm)}{note}")
        elif event.get("kind") == "ota":
            self.log(f"ota: version {event['version']} size {event['size']} sha256 {event['sha256'][:12]}... url {event['url']}")
            if self.ota_mode != "ignore":
                asyncio.get_running_loop().create_task(self.perform_ota(event, fail=self.ota_mode == "fail"))
        else:
            self.log("unknown event:", event)
        loop = asyncio.get_running_loop()
        loop.create_task(self._notify())

    async def _notify(self) -> None:
        async with self._event_cond:
            self._event_cond.notify_all()

    @staticmethod
    def describe(vm: dict[str, Any]) -> str:
        parts = [vm["status"] or "-", "ON" if vm["on"] else "off"]
        if not vm["available"]:
            parts.append("UNAVAILABLE")
        if vm["detail"]:
            parts.append(repr(vm["detail"]))
        if vm["moving"]:
            parts.append("moving")
        if vm["brightness"] >= 0:
            parts.append(f"bri={vm['brightness']}")
        if vm["mode"]:
            parts.append(f"mode={vm['mode']} modes={vm['modes']} target={vm['target']} cur={vm['current']} {vm['min']}-{vm['max']}/{vm['step']}{vm['unit']} hum={vm['humidity']}")
        return " ".join(parts)

    def mark(self) -> int:
        """Event index to pass as `since` (take it before sending an action)."""
        return len(self.events)

    async def wait_for(self, predicate: Any, timeout: float = 10.0, since: int | None = None) -> dict[str, Any]:
        """Wait until predicate(event) is true for an event at index >= since."""
        start = len(self.events) if since is None else since

        async def _wait() -> dict[str, Any]:
            idx = start
            async with self._event_cond:
                while True:
                    while idx < len(self.events):
                        ev = self.events[idx]
                        idx += 1
                        if predicate(ev):
                            return ev
                    await self._event_cond.wait()

        return await asyncio.wait_for(_wait(), timeout)

    async def wait_tile(
        self, tile_id: str, check: Any, timeout: float = 10.0, since: int | None = None
    ) -> dict[str, Any]:
        """Wait for a states event (index >= since) for tile_id whose view model satisfies check(vm)."""
        ev = await self.wait_for(
            lambda e: e.get("kind") == "states" and tile_id in e["tiles"] and check(e["tiles"][tile_id]),
            timeout,
            since,
        )
        return ev["tiles"][tile_id]

    async def download(self, url: str, token: str | None = "") -> tuple[int, bytes, dict[str, str]]:
        """GET a firmware URL like the screen does. token None = no auth header."""
        assert self.session is not None
        headers = {}
        if token is not None:
            headers["Authorization"] = f"Bearer {token or self.tokens['access_token']}"
        async with self.session.get(self.url + url, headers=headers) as resp:
            return resp.status, await resp.read(), dict(resp.headers)

    async def ota_status(self, state: str, progress: int = 0, error: str | None = None) -> Any:
        assert self.ws is not None
        payload: dict[str, Any] = {"screen_id": self.screen_id, "state": state, "progress": progress}
        if error is not None:
            payload["error"] = error
        self.log(f"ota_status {state} {progress}%" + (f" ({error})" if error else ""))
        return await self.ws.call("esp32_screen/ota_status", **payload)

    async def perform_ota(self, event: dict[str, Any], *, fail: bool = False) -> None:
        """What the firmware does: download, verify size + sha256, restart, re-register."""
        import hashlib

        await self.ota_status("downloading", 0)
        status, body, _ = await self.download(event["url"])
        if status != 200:
            await self.ota_status("failed", 0, f"HTTP {status}")
            return
        for pct in (25, 50, 75, 100):
            await self.ota_status("downloading", pct)
            await asyncio.sleep(0.2)
        await self.ota_status("verifying", 100)
        if fail or len(body) != event["size"] or hashlib.sha256(body).hexdigest() != event["sha256"]:
            await self.ota_status("failed", 100, "sha256 mismatch" if not fail else "simulated failure")
            return
        await self.ota_status("restarting", 100)
        # "Reboot": drop the connection, come back with the new version.
        await self.stop()
        await asyncio.sleep(1)
        self.sw_version = event["version"]
        await self.start()

    async def action(self, tile_id: str, action: str, value: Any = None) -> Any:
        assert self.ws is not None
        payload: dict[str, Any] = {"screen_id": self.screen_id, "tile_id": tile_id, "action": action}
        if value is not None:
            payload["value"] = value
        self.log(f"action {tile_id} {action}" + (f" {value!r}" if value is not None else ""))
        return await self.ws.call("esp32_screen/action", **payload)


def parse_value(raw: str) -> Any:
    try:
        return int(raw)
    except ValueError:
        pass
    try:
        return float(raw)
    except ValueError:
        return raw


async def run_cli(args: argparse.Namespace) -> int:
    sim = ScreenSim(args.url, args.user, args.password, args.screen_id, name=args.name, sw_version=args.sw_version)
    sim.ota_mode = args.ota
    try:
        await sim.start()
    except (LoginError, CommandError, aiohttp.ClientError) as err:
        print(f"error: {err}", file=sys.stderr)
        await sim.stop()
        return 1
    try:
        await asyncio.sleep(1)
        for spec in args.action:
            tile_id, action, *rest = spec.split(":", 2)
            try:
                await sim.action(tile_id, action, parse_value(rest[0]) if rest else None)
                sim.log("  -> ok")
            except CommandError as err:
                sim.log(f"  -> error {err}")
            await asyncio.sleep(args.delay)
        if args.interactive:
            loop = asyncio.get_running_loop()
            while True:
                line = await loop.run_in_executor(None, sys.stdin.readline)
                if not line or line.strip() in ("quit", "exit"):
                    break
                parts = line.split()
                if len(parts) < 2:
                    print("usage: TILE ACTION [VALUE]")
                    continue
                try:
                    await sim.action(parts[0], parts[1], parse_value(parts[2]) if len(parts) > 2 else None)
                    sim.log("  -> ok")
                except CommandError as err:
                    sim.log(f"  -> error {err}")
        else:
            await asyncio.sleep(args.duration)
    finally:
        await sim.stop()
    return 0


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--url", default="http://localhost:8123")
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "dev"))
    import local  # dev-config/dev-local.json, see homeassistant/dev/local.py

    parser.add_argument("--user", default=local.screen_user()[0])
    parser.add_argument("--password", default=local.screen_user()[1])
    parser.add_argument("--screen-id", default="AABBCCDDEEFF")
    parser.add_argument("--name", default="Simulated Screen")
    parser.add_argument("--action", action="append", default=[], metavar="TILE:ACTION[:VALUE]")
    parser.add_argument("--delay", type=float, default=2.0, help="seconds between --action")
    parser.add_argument("--duration", type=float, default=5.0, help="seconds to keep listening")
    parser.add_argument("--interactive", action="store_true")
    parser.add_argument("--sw-version", default="0.0.0-sim", help="version sent in register")
    parser.add_argument("--ota", choices=("ignore", "apply", "fail"), default="ignore",
                        help="on an ota event: do nothing, perform the update, or report a failure")
    sys.exit(asyncio.run(run_cli(parser.parse_args())))


if __name__ == "__main__":
    main()
