#!/usr/bin/env bash
set -euo pipefail

MODEL="${1:-base}"
DEST_DIR="${2:-src-tauri/resources}"

REPO="ggerganov/whisper.cpp"
BASE_URL="https://huggingface.co/${REPO}/resolve/main"

case "$MODEL" in
  tiny)   FILE="ggml-tiny.bin" ;;
  base)   FILE="ggml-base.bin" ;;
  small)  FILE="ggml-small.bin" ;;
  medium) FILE="ggml-medium.bin" ;;
  *)
    echo "Usage: $0 [tiny|base|small|medium] [dest_dir]"
    exit 1
    ;;
esac

mkdir -p "$DEST_DIR"
URL="${BASE_URL}/${FILE}"
OUTPUT="${DEST_DIR}/${FILE}"

if [ -f "$OUTPUT" ]; then
  echo "Model already exists: ${OUTPUT}"
  exit 0
fi

echo "Downloading ${FILE} from Hugging Face..."
curl -L --progress-bar -o "$OUTPUT" "$URL"
echo "Downloaded to: ${OUTPUT} ($(du -h "$OUTPUT" | cut -f1))"
