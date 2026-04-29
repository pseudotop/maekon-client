#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT_DIR"

OPENAPI_PATH="docs/contracts/oneshim-web.v1.openapi.yaml"
TMP_OPENAPI="$(mktemp)"
trap 'rm -f "$TMP_OPENAPI"' EXIT

if [[ ! -f "$OPENAPI_PATH" ]]; then
  echo "[http-openapi] missing file: $OPENAPI_PATH" >&2
  echo "[http-openapi] run ./scripts/generate-http-openapi.sh to bootstrap it" >&2
  exit 1
fi

./scripts/generate-http-openapi.sh "$TMP_OPENAPI" >/dev/null

if ! diff -u "$OPENAPI_PATH" "$TMP_OPENAPI"; then
  echo "[http-openapi] snapshot drift detected: $OPENAPI_PATH" >&2
  echo "[http-openapi] run ./scripts/generate-http-openapi.sh and commit updated snapshot" >&2
  exit 1
fi

echo "[http-openapi] snapshot is up to date"
