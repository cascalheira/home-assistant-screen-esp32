#!/usr/bin/env bash
# Start (or restart) the development Home Assistant in Docker.
#
#   homeassistant/dev/run-ha.sh
#
# Config lives in homeassistant/dev-config/ (git-ignored). The integration is
# bind-mounted read-only, so after editing it `docker restart esp32-screen-ha`.
# Port 8123 is published on all interfaces so a screen on the LAN can reach it.
set -euo pipefail

HA_DIR="$(cd "$(dirname "$0")/.." && pwd)"
CONFIG="$HA_DIR/dev-config"
NAME=esp32-screen-ha
IMAGE=ghcr.io/home-assistant/home-assistant:stable

mkdir -p "$CONFIG/custom_components/esp32_screen"
cp "$HA_DIR/dev/configuration.yaml" "$CONFIG/configuration.yaml"

docker rm -f "$NAME" >/dev/null 2>&1 || true
docker run -d \
  --name "$NAME" \
  --restart unless-stopped \
  -p 8123:8123 \
  -v "$CONFIG:/config" \
  -v "$HA_DIR/custom_components/esp32_screen:/config/custom_components/esp32_screen:ro" \
  "$IMAGE"

echo "Waiting for Home Assistant on http://localhost:8123 ..."
for _ in $(seq 1 120); do
  if curl -fsS -o /dev/null http://localhost:8123/manifest.json 2>/dev/null; then
    echo "Home Assistant is up."
    exit 0
  fi
  sleep 2
done
echo "Home Assistant did not come up; see: docker logs $NAME" >&2
exit 1
