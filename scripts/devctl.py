#!/usr/bin/env python3
"""Drive the board's debug console over the CH340 serial port.

On macOS, opening the CH340 port pulses DTR/RTS and the auto-reset circuit
reboots the board. So one long-running `serve` process opens the port once,
appends everything the board prints to a log file, and forwards command lines
it receives on a local TCP socket. The other subcommands talk to that server.

Usage (run with uv so pyserial/pillow are available):
  uv run --with pyserial --with pillow scripts/devctl.py serve &   # keep running
  uv run --with pyserial --with pillow scripts/devctl.py log 5      # last 5 s of output
  uv run --with pyserial --with pillow scripts/devctl.py cmd "tap 400 240" 2
  uv run --with pyserial --with pillow scripts/devctl.py shot out.png
  uv run --with pyserial --with pillow scripts/devctl.py reset 8

Stop the server before flashing; espflash needs the port.
Set ESP_PORT to override the default port.
"""

import base64
import glob
import os
import re
import socket
import sys
import threading
import time

import serial

PORT = os.environ.get("ESP_PORT") or next(
    iter(sorted(glob.glob("/dev/cu.usbserial-*") + glob.glob("/dev/ttyUSB*"))), "/dev/ttyUSB0"
)
BAUD = 115200
LOG = os.environ.get("ESP_LOG", "/tmp/esp32-screen-serial.log")
CONTROL = ("127.0.0.1", 7766)


def open_port():
    s = serial.Serial()
    s.port = PORT
    s.baudrate = BAUD
    s.timeout = 0.1
    s.dtr = False  # GPIO0 high: normal boot
    s.rts = False  # EN high: not in reset
    s.open()
    return s


def read_for(s, seconds, until=None):
    buf = b""
    end = time.time() + seconds
    while time.time() < end:
        chunk = s.read(4096)
        if chunk:
            buf += chunk
            if until and until in buf:
                break
    return buf


def cmd_serve():
    s = open_port()
    lock = threading.Lock()
    log = open(LOG, "ab", buffering=0)

    def pump():
        while True:
            chunk = s.read(4096)
            if chunk:
                log.write(chunk)

    threading.Thread(target=pump, daemon=True).start()
    srv = socket.create_server(CONTROL)
    print(f"serving {PORT} on {CONTROL[0]}:{CONTROL[1]}, log {LOG}", flush=True)
    while True:
        conn, _ = srv.accept()
        with conn:
            data = conn.recv(4096).decode()
            with lock:
                if data.startswith("!reset"):
                    s.rts = True
                    time.sleep(0.15)
                    s.rts = False
                else:
                    s.write(data.encode())
            conn.sendall(b"ok")


def send_to_server(line):
    with socket.create_connection(CONTROL, timeout=5) as c:
        c.sendall(line.encode())
        c.recv(16)


def log_size():
    return os.path.getsize(LOG) if os.path.exists(LOG) else 0


def read_log_from(offset, seconds, until=None):
    end = time.time() + seconds
    while True:
        with open(LOG, "rb") as f:
            f.seek(offset)
            data = f.read()
        if (until and until in data) or time.time() >= end:
            return data
        time.sleep(0.2)


def cmd_log(seconds):
    start = max(0, log_size() - 20000)
    data = read_log_from(start, 0)
    sys.stdout.write(data.decode("utf-8", "replace")[-6000:])
    if float(seconds) > 0:
        offset = log_size()
        sys.stdout.write(read_log_from(offset, float(seconds)).decode("utf-8", "replace"))


def cmd_reset(seconds):
    offset = log_size()
    send_to_server("!reset")
    sys.stdout.write(read_log_from(offset, float(seconds)).decode("utf-8", "replace"))


def cmd_send(line, seconds="1"):
    offset = log_size()
    send_to_server(line + "\n")
    sys.stdout.write(read_log_from(offset, float(seconds)).decode("utf-8", "replace"))


def cmd_shot(path, attempts=3):
    # Log lines from other tasks can still cut into the dump: retry.
    for attempt in range(int(attempts)):
        try:
            return take_shot(path)
        except ShotCorrupted as e:
            print(f"{e}, retrying" if attempt + 1 < int(attempts) else str(e), file=sys.stderr)
    sys.exit(1)


class ShotCorrupted(Exception):
    pass


def take_shot(path):
    from PIL import Image

    offset = log_size()
    send_to_server("shot\n")
    raw = read_log_from(offset, 90, until=b"SHOT-END")
    text = raw.decode("utf-8", "replace")
    m = re.search(r"SHOT-BEGIN (\d+) (\d+) (\d+)\s*\n(.*?)SHOT-END", text, re.S)
    if not m:
        sys.exit("no screenshot in output:\n" + text[-2000:])
    w, h, n = int(m[1]), int(m[2]), int(m[3])
    body = "".join(line.strip() for line in m[4].splitlines() if re.fullmatch(r"[A-Za-z0-9+/=]+", line.strip()))
    rle = base64.b64decode(body)
    if len(rle) != n:
        raise ShotCorrupted(f"screenshot corrupted: got {len(rle)} bytes, expected {n}")
    pixels = bytearray()
    for i in range(0, len(rle), 3):
        run, value = rle[i], rle[i + 1] | rle[i + 2] << 8
        r = (value >> 11 & 0x1F) * 255 // 31
        g = (value >> 5 & 0x3F) * 255 // 63
        b = (value & 0x1F) * 255 // 31
        pixels += bytes((r, g, b)) * run
    if len(pixels) != w * h * 3:
        raise ShotCorrupted(f"screenshot has {len(pixels) // 3} pixels, expected {w * h}")
    Image.frombytes("RGB", (w, h), bytes(pixels)).save(path)
    print(f"saved {path} ({w}x{h}, {n} bytes compressed)")


if __name__ == "__main__":
    commands = {
        "serve": cmd_serve,
        "log": cmd_log,
        "reset": cmd_reset,
        "cmd": cmd_send,
        "shot": cmd_shot,
    }
    if len(sys.argv) < 2 or sys.argv[1] not in commands:
        sys.exit(__doc__)
    commands[sys.argv[1]](*sys.argv[2:])
