#!/bin/bash
# Runs cargo with the Xtensa ("esp") Rust toolchain.
#
# Homebrew's rustc sits earlier in PATH than rustup's proxy on this machine,
# so cargo would otherwise pick a compiler without Xtensa support.
#
# Usage: scripts/cargo-esp.sh build --release
#        scripts/cargo-esp.sh run --release        (flash + serial monitor)
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
export ESP_IDF_SDKCONFIG_DEFAULTS="$PWD/sdkconfig.defaults;$PWD/.embuild/sdkconfig.partitions"

exec "$HOME/.cargo/bin/cargo" +esp "$@"
