#!/bin/bash
# Runs the firmware's host tests (host-tests/) with standard Rust.
#
# The repository's .cargo/config.toml builds for the ESP32-S3 by default, so
# name the computer's own target explicitly.
set -euo pipefail
cd "$(dirname "$0")/../host-tests"

# Prefer rustup's tools over e.g. Homebrew's rustc earlier in PATH.
if [ -x "$HOME/.cargo/bin/cargo" ]; then CARGO="$HOME/.cargo/bin/cargo"; else CARGO=cargo; fi
HOST=$("$(rustup which rustc)" -vV | sed -n 's/^host: //p')
exec "$CARGO" test --target "$HOST" "$@"
