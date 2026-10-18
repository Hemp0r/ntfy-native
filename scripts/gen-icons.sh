#!/usr/bin/env bash
# Regenerates the committed app icon rasters from assets/icon.svg (macOS only):
#   assets/AppIcon.icns  macOS bundle icon (Finder, notifications)
#   assets/icon.ico      Windows executable icon (16, 32, 48, 256 px)
#   assets/icon-256.png  Windows toast/Settings icon (IconUri)
# Usage: scripts/gen-icons.sh
set -euo pipefail
cd "$(dirname "$0")/.."

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

master="$tmp/icon-1024.png"
# Newer sips rasterizes SVG directly; Quick Look is the fallback.
if ! sips -s format png assets/icon.svg --out "$master" >/dev/null 2>&1; then
  qlmanage -t -s 1024 -o "$tmp" assets/icon.svg >/dev/null
  mv "$tmp/icon.svg.png" "$master"
fi

resize() { sips -z "$1" "$1" "$master" --out "$2" >/dev/null; }

iconset="$tmp/AppIcon.iconset"
mkdir "$iconset"
for size in 16 32 128 256 512; do
  resize "$size" "$iconset/icon_${size}x${size}.png"
  resize $((size * 2)) "$iconset/icon_${size}x${size}@2x.png"
done
iconutil -c icns "$iconset" -o assets/AppIcon.icns

resize 256 assets/icon-256.png

for size in 16 32 48 256; do
  resize "$size" "$tmp/ico-$size.png"
done
# ICO with embedded PNGs (supported since Windows Vista).
python3 -I - assets/icon.ico "$tmp"/ico-{16,32,48,256}.png <<'PY'
import struct, sys

out, pngs = sys.argv[1], [open(p, "rb").read() for p in sys.argv[2:]]
header = struct.pack("<HHH", 0, 1, len(pngs))
entries, offset = b"", 6 + 16 * len(pngs)
for png in pngs:
    assert png[:8] == b"\x89PNG\r\n\x1a\n" and png[12:16] == b"IHDR"
    w, h = struct.unpack(">II", png[16:24])
    entries += struct.pack("<BBBBHHII", w % 256, h % 256, 0, 0, 1, 32, len(png), offset)
    offset += len(png)
with open(out, "wb") as f:
    f.write(header + entries + b"".join(pngs))
PY

ls -l assets/AppIcon.icns assets/icon.ico assets/icon-256.png
