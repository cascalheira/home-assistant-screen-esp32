#!/bin/bash
# Builds and flashes the firmware over USB.
#
# Uses the bootloader ESP-IDF built from sdkconfig.defaults (it has rollback
# support, which espflash's bundled bootloader lacks) and erases `otadata`, so
# the board boots this image from ota_0 even if an earlier over-the-air update
# had switched it to ota_1. Settings in `nvs` (Wi-Fi, Home Assistant login)
# are kept.
#
# Usage: scripts/flash.sh               (ESPFLASH_PORT overrides the port)
#        BOARD=p4 scripts/flash.sh      (the ESP32-P4 board)
#        FIRST_FLASH=1 scripts/flash.sh (also erase nvs: once, after the
#                                        factory firmware, whose data sits there)
set -euo pipefail
cd "$(dirname "$0")/.."

scripts/cargo-esp.sh build --release
case "${BOARD:-s3}" in
  s3)
    OUT=target/xtensa-esp32s3-espidf/release
    # The CH340 bridge drops packets above 230400 baud through a USB dock.
    BAUD=230400
    # Port: $ESPFLASH_PORT, else the first USB-serial adapter found.
    PORT="${ESPFLASH_PORT:-$( (ls /dev/cu.usbserial-* /dev/ttyUSB* 2>/dev/null || true) | head -1)}"
    ;;
  p4)
    OUT=target/riscv32imafc-esp-espidf/release
    BAUD=921600 # the chip's own USB: the rate is nominal
    # Port: $ESPFLASH_PORT, else the Espressif USB Serial/JTAG device (other
    # Espressif-based gadgets share the vendor id, so match the product too).
    PORT="${ESPFLASH_PORT:-$(uv run --quiet --with pyserial python -c '
from serial.tools import list_ports
print(next((p.device for p in list_ports.comports()
            if p.vid == 0x303A and "JTAG" in (p.product or p.description or "")), ""))')}"
    ;;
esac
if [ "${BOARD:-s3}" = p4 ] && [ -n "$PORT" ]; then
  # Turn the panel off properly first: an LCD left on without a video signal
  # while flashing flickers at the edges for a while afterwards. (Needs the
  # running firmware's debug console; harmless when it has none.)
  uv run --quiet --with pyserial python -c '
import serial, sys, time
try:
    s = serial.Serial(sys.argv[1], 115200, timeout=1)
    s.write(b"\npaneloff\n"); s.flush()
    end = time.time() + 1.5
    reply = b""
    while time.time() < end and b"PANEL OFF" not in reply:
        reply += s.read(256)
    print("panel off" if b"PANEL OFF" in reply else "panel off: no reply")
    s.close()
except Exception as e:
    print("panel off skipped:", e)' "$PORT" || true
fi
ERASE=otadata
[ -n "${FIRST_FLASH:-}" ] && ERASE=otadata,nvs,phy_init
espflash flash \
  ${PORT:+--port "$PORT"} \
  --baud "$BAUD" \
  --flash-size 16mb \
  --partition-table partitions.csv \
  --bootloader "$OUT/bootloader.bin" \
  --erase-parts "$ERASE" \
  "$OUT/esp32-screen"
