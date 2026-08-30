#!/usr/bin/env bash
# Build the two explicitly supported WiX locales from the same release payload.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CLIENT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
TARGET="${1:-x86_64-pc-windows-msvc}"
TARGET_BIN_DIR="${2:-../target/${TARGET}/release}"
LOCALE="${3:-all}"

build_locale() {
  local culture="$1"
  local language="$2"
  local codepage="$3"

  (
    cd "$CLIENT_ROOT/src-tauri"
    ../scripts/cargo-cache.sh wix \
      -p maekon-app \
      --nocapture \
      --target "$TARGET" \
      --no-build \
      --target-bin-dir "$TARGET_BIN_DIR" \
      --culture "$culture" \
      --locale "wix/${culture}.wxl" \
      --name "maekon-app-${culture}" \
      --compiler-arg "-dInstallerLanguage=${language}" \
      --compiler-arg "-dInstallerCodepage=${codepage}"
  )
}

case "$LOCALE" in
  all)
    rm -f "$CLIENT_ROOT/target/wix/"*.msi "$CLIENT_ROOT/src-tauri/target/wix/"*.msi
    build_locale ko-KR 1042 949
    build_locale en-US 1033 1252
    ;;
  ko-KR)
    build_locale ko-KR 1042 949
    ;;
  en-US)
    build_locale en-US 1033 1252
    ;;
  *)
    echo "Unsupported MSI locale: $LOCALE (expected all, ko-KR, or en-US)" >&2
    exit 2
    ;;
esac
