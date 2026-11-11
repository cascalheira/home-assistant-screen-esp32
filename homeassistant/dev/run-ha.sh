#!/usr/bin/env bash
# Start (or restart) the development Home Assistant in Docker.
#
#   homeassistant/dev/run-ha.sh
#
# Config lives in homeassistant/dev-config/ (git-ignored). The integration
# (custom_components/esp32_screen at the repo root) is bind-mounted read-only, so after editing it `docker restart esp32-screen-ha`.
# Port 8123 is published on all interfaces so a screen on the LAN can reach it.
# ESP32_SCREEN_HA_NAME / ESP32_SCREEN_HA_PORT change the container name and port.
set -euo pipefail

HA_DIR="$(cd "$(dirname "$0")/.." && pwd)"
REPO="$(cd "$HA_DIR/.." && pwd)"
CONFIG="$HA_DIR/dev-config"
NAME="${ESP32_SCREEN_HA_NAME:-esp32-screen-ha}"
PORT="${ESP32_SCREEN_HA_PORT:-8123}"
IMAGE=ghcr.io/home-assistant/home-assistant:stable

mkdir -p "$CONFIG/custom_components/esp32_screen"
cp "$HA_DIR/dev/configuration.yaml" "$CONFIG/configuration.yaml"

# A test RTSP stream for the doorbell camera (rtsp://<host>:18554/doorbell).
docker rm -f esp32-screen-rtsp >/dev/null 2>&1 || true
docker run -d --name esp32-screen-rtsp --restart unless-stopped -p 18554:8554 \
  -e MTX_PATHS_DOORBELL_RUNONINIT="ffmpeg -re -f lavfi -i testsrc2=size=1280x720:rate=10 -c:v libx264 -preset ultrafast -tune zerolatency -g 20 -f rtsp rtsp://localhost:8554/doorbell" \
  -e MTX_PATHS_DOORBELL_RUNONINITRESTART=yes \
  bluenviron/mediamtx:latest-ffmpeg >/dev/null

docker rm -f "$NAME" >/dev/null 2>&1 || true
docker run -d \
  --name "$NAME" \
  --restart unless-stopped \
  -p "$PORT:8123" \
  -v "$CONFIG:/config" \
  -v "$REPO/custom_components/esp32_screen:/config/custom_components/esp32_screen:ro" \
  "$IMAGE"

echo "Waiting for Home Assistant on http://localhost:$PORT ..."
for _ in $(seq 1 120); do
  if curl -fsS -o /dev/null "http://localhost:$PORT/manifest.json" 2>/dev/null; then
    echo "Home Assistant is up."
    exit 0
  fi
  sleep 2
done
echo "Home Assistant did not come up; see: docker logs $NAME" >&2
exit 1
