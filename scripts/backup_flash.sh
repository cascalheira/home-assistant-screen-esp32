#!/bin/bash
# Back up the full 16 MB flash of the board in 1 MB chunks, retrying each
# chunk on serial errors, then join the chunks into one image.
#
# Usage: scripts/backup_flash.sh [port] [output.bin]
# Restore: esptool --port <port> --baud 230400 write-flash 0 <output.bin>
set -u

PORT="${1:-${ESPFLASH_PORT:-$(ls /dev/cu.usbserial-* /dev/ttyUSB* 2>/dev/null | head -1)}}"
OUT="${2:-backup/factory-firmware-16MB.bin}"
CHUNK=$((0x100000))
TOTAL=$((0x1000000))
DIR="$(dirname "$OUT")/chunks"
mkdir -p "$DIR"

offset=0
while [ "$offset" -lt "$TOTAL" ]; do
  part="$DIR/$(printf '%08x' "$offset").bin"
  if [ -f "$part" ] && [ "$(stat -f %z "$part")" -eq "$CHUNK" ]; then
    offset=$((offset + CHUNK)); continue
  fi
  ok=0
  for baud in 230400 230400 115200 115200 115200; do
    rm -f "$part"
    if uvx esptool --port "$PORT" --baud "$baud" read-flash "$offset" "$CHUNK" "$part" >/dev/null 2>&1 \
       && [ "$(stat -f %z "$part" 2>/dev/null)" = "$CHUNK" ]; then
      ok=1; break
    fi
    echo "retry chunk $(printf '0x%x' "$offset") (failed at $baud baud)"
  done
  if [ "$ok" -ne 1 ]; then echo "FAILED at offset $(printf '0x%x' "$offset")"; exit 1; fi
  echo "chunk $(printf '0x%x' "$offset") ok"
  offset=$((offset + CHUNK))
done

cat "$DIR"/*.bin > "$OUT"
size=$(stat -f %z "$OUT")
if [ "$size" -ne "$TOTAL" ]; then echo "FAILED: joined size $size"; exit 1; fi
rm -rf "$DIR"
echo "BACKUP OK: $OUT ($size bytes) sha256 $(shasum -a 256 "$OUT" | cut -d' ' -f1)"
