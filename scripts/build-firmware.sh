#!/bin/bash
# Builds the firmware and writes the update image dist/esp32-screen-<version>.bin
# (an ESP-IDF app image, the format Home Assistant serves and the SD-card
# updater reads).
#
# Usage: scripts/build-firmware.sh [--dev] [--publish DIR]
#   --dev           keep the serial debug console (left out of release images)
#   --publish DIR   also copy the image into DIR, e.g. Home Assistant's
#                   <config>/esp32_screen/firmware folder
set -euo pipefail
cd "$(dirname "$0")/.."

PUBLISH=""
FEATURES=(--no-default-features)
while [ $# -gt 0 ]; do
  case "$1" in
    --dev) FEATURES=(); shift ;;
    --publish) PUBLISH="${2:?--publish needs a folder}"; shift 2 ;;
    *) echo "unknown option $1" >&2; exit 2 ;;
  esac
done

scripts/cargo-esp.sh build --release ${FEATURES[@]+"${FEATURES[@]}"}
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
mkdir -p dist
IMAGE="dist/esp32-screen-$VERSION.bin"
espflash save-image --chip esp32s3 --flash-size 16mb --partition-table partitions.csv \
  target/xtensa-esp32s3-espidf/release/esp32-screen "$IMAGE" >/dev/null
echo "$IMAGE ($(wc -c < "$IMAGE" | tr -d ' ') bytes)"

if [ -n "$PUBLISH" ]; then
  cp "$IMAGE" "$PUBLISH/"
  echo "published to $PUBLISH"
fi
