#!/bin/bash
# Runs cargo with the Xtensa ("esp") Rust toolchain.
#
# Homebrew's rustc sits earlier in PATH than rustup's proxy on this machine,
# so cargo would otherwise pick a compiler without Xtensa support.
#
# Usage: scripts/cargo-esp.sh build --release
#        scripts/cargo-esp.sh run --release        (flash + serial monitor)
#        BOARD=p4 scripts/cargo-esp.sh build --release
#
# BOARD picks the hardware: s3 (default; ESP32-S3, 800x480 RGB panel) or p4
# (ESP32-P4, 1024x600 MIPI-DSI panel, Wi-Fi through an ESP32-C6).
set -euo pipefail

cd "$(dirname "$0")/.."

# shellcheck disable=SC1091
[ -f "$HOME/export-esp.sh" ] && source "$HOME/export-esp.sh"

RUSTC="$(rustup which --toolchain esp rustc)"
RUSTDOC="$(rustup which --toolchain esp rustdoc)"
export RUSTC RUSTDOC
export PATH="$(dirname "$RUSTC"):$PATH"

# ESP-IDF resolves CONFIG_PARTITION_TABLE_CUSTOM_FILENAME relative to its own
# build folder, so hand it an absolute path through a generated defaults file.
mkdir -p .embuild
printf 'CONFIG_PARTITION_TABLE_CUSTOM=y\nCONFIG_PARTITION_TABLE_CUSTOM_FILENAME="%s"\n' \
  "$PWD/partitions.csv" > .embuild/sdkconfig.partitions
case "${BOARD:-s3}" in
  s3) MCU=esp32s3; TARGET=xtensa-esp32s3-espidf ;;
  # ESP-IDF 5.5.3's MIPI-DSI driver garbles this panel's picture; v5.5.6 has
  # the PHY and clock-lane fixes the factory firmware's ESP-IDF has.
  p4) MCU=esp32p4; TARGET=riscv32imafc-esp-espidf; export ESP_IDF_VERSION=v5.5.6 ;;
  *) echo "BOARD must be s3 or p4" >&2; exit 1 ;;
esac
export MCU
export ESP_IDF_SDKCONFIG_DEFAULTS="$PWD/sdkconfig.defaults;$PWD/sdkconfig.defaults.$MCU;$PWD/.embuild/sdkconfig.partitions"

# Subcommands that build get the board's target; others (fmt, clean) don't take it.
case "${1:-}" in
  build|run|check|clippy|rustc|doc) exec "$HOME/.cargo/bin/cargo" +esp "$1" --target "$TARGET" "${@:2}" ;;
  *) exec "$HOME/.cargo/bin/cargo" +esp "$@" ;;
esac
