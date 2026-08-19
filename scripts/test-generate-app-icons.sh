#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CLIENT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
OUTPUT_DIR="$(mktemp -d "${TMPDIR:-/tmp}/maekon-generated-icons.XXXXXX")"
trap 'rm -rf "$OUTPUT_DIR"' EXIT

if rg -n 'magick[^\n]*(\.svg|SOURCE_SVG|TRAY_SOURCE_SVG)' "$SCRIPT_DIR/generate-app-icons.sh"; then
  echo "[ERROR] ImageMagick must not receive SVG input." >&2
  exit 1
fi

"$SCRIPT_DIR/generate-app-icons.sh" \
  "$CLIENT_ROOT/assets/brand/logo-icon.svg" \
  "$OUTPUT_DIR" \
  "$CLIENT_ROOT/assets/brand/tray-template.svg"

"${PYTHON_BIN:-python3}" - "$OUTPUT_DIR" <<'PY'
import sys
from pathlib import Path
from PIL import Image

output_dir = Path(sys.argv[1])
expected = {
    "32x32.png": (32, 32),
    "128x128.png": (128, 128),
    "128x128@2x.png": (256, 256),
    "icon.png": (1024, 1024),
    "dock_icon.png": (1024, 1024),
    "tray_icon.png": (22, 22),
    "tray_icon@2x.png": (44, 44),
}

for filename, size in expected.items():
    with Image.open(output_dir / filename) as image:
        assert image.size == size, (filename, image.size)

with Image.open(output_dir / "icon.ico") as image:
    assert image.format == "ICO", image.format
    assert (256, 256) in image.info["sizes"], image.info["sizes"]

with Image.open(output_dir / "icon.icns") as image:
    assert image.format == "ICNS", image.format
    assert image.size == (1024, 1024), image.size

print("[OK] Generated PNG, ICO, and ICNS artifacts have the expected formats and dimensions.")
PY
