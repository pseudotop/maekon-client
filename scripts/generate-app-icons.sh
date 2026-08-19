#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CLIENT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
RENDERER_DIR="$SCRIPT_DIR/icon-renderer"
SOURCE_SVG="${1:-$CLIENT_ROOT/assets/brand/logo-icon.svg}"
OUTPUT_DIR="${2:-$CLIENT_ROOT/src-tauri/icons}"
TRAY_SOURCE_SVG="${3:-$CLIENT_ROOT/assets/brand/tray-template.svg}"
PYTHON_BIN="${PYTHON_BIN:-python3}"

if [[ ! -f "$SOURCE_SVG" ]]; then
  echo "[ERROR] Source logo not found: $SOURCE_SVG" >&2
  exit 1
fi

if ! command -v node >/dev/null 2>&1; then
  echo "[ERROR] Node.js is required." >&2
  exit 1
fi

if [[ ! -d "$RENDERER_DIR/node_modules/sharp" ]]; then
  echo "[ERROR] Icon renderer dependencies are missing." >&2
  echo "        Run: npm ci --prefix $RENDERER_DIR" >&2
  exit 1
fi

if ! command -v magick >/dev/null 2>&1; then
  echo "[ERROR] ImageMagick (magick) is required." >&2
  exit 1
fi

if ! command -v "$PYTHON_BIN" >/dev/null 2>&1; then
  echo "[ERROR] Python is required: $PYTHON_BIN" >&2
  exit 1
fi

mkdir -p "$OUTPUT_DIR"

node "$RENDERER_DIR/render-app-icon-pngs.mjs" "$SOURCE_SVG" "$OUTPUT_DIR" "$TRAY_SOURCE_SVG"
magick "$OUTPUT_DIR/icon.png" -define icon:auto-resize=256,128,64,48,32,16 "$OUTPUT_DIR/icon.ico"

if [[ ! -f "$TRAY_SOURCE_SVG" ]]; then
  echo "[WARN] Tray source not found, skipping tray icons: $TRAY_SOURCE_SVG" >&2
fi

"$PYTHON_BIN" - "$OUTPUT_DIR/icon.png" "$OUTPUT_DIR/icon.icns" <<'PY'
import sys
from PIL import Image

png_path = sys.argv[1]
icns_path = sys.argv[2]

img = Image.open(png_path).convert("RGBA")
img.save(
    icns_path,
    format="ICNS",
    sizes=[(16, 16), (32, 32), (64, 64), (128, 128), (256, 256), (512, 512), (1024, 1024)],
)
PY

echo "[OK] Generated:"
echo "  - $OUTPUT_DIR/32x32.png"
echo "  - $OUTPUT_DIR/128x128.png"
echo "  - $OUTPUT_DIR/128x128@2x.png"
echo "  - $OUTPUT_DIR/dock_icon.png"
echo "  - $OUTPUT_DIR/icon.png"
echo "  - $OUTPUT_DIR/icon.ico"
echo "  - $OUTPUT_DIR/icon.icns"
if [[ -f "$TRAY_SOURCE_SVG" ]]; then
  echo "  - $OUTPUT_DIR/tray_icon.png"
  echo "  - $OUTPUT_DIR/tray_icon@2x.png"
fi
