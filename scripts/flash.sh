#!/bin/bash
# Builds and flashes the firmware over USB.
#
# Uses the bootloader ESP-IDF built from sdkconfig.defaults (it has rollback
# support, which espflash's bundled bootloader lacks) and erases `otadata`, so
# the board boots this image from ota_0 even if an earlier over-the-air update
# had switched it to ota_1. Settings in `nvs` (Wi-Fi, Home Assistant login)
# are kept.
#
# Usage: scripts/flash.sh          (ESPFLASH_PORT overrides the port)
set -euo pipefail
cd "$(dirname "$0")/.."

scripts/cargo-esp.sh build --release
# Port: $ESPFLASH_PORT, else the first USB-serial adapter found.
PORT="${ESPFLASH_PORT:-$(ls /dev/cu.usbserial-* /dev/ttyUSB* 2>/dev/null | head -1)}"
OUT=target/xtensa-esp32s3-espidf/release
espflash flash \
  ${PORT:+--port "$PORT"} \
  --baud 230400 \
  --flash-size 16mb \
  --partition-table partitions.csv \
  --bootloader "$OUT/bootloader.bin" \
  --erase-parts otadata \
  "$OUT/esp32-screen"
