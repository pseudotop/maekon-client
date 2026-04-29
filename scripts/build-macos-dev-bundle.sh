#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
APP_PATH="$ROOT_DIR/target/debug/bundle/macos/Maekon Dev.app"
ENTITLEMENTS="$ROOT_DIR/src-tauri/assets/oneshim.entitlements"
SIGN_IDENTITY="${ONESHIM_DEV_CODESIGN_IDENTITY:--}"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "error: macOS dev bundle signing is only available on Darwin." >&2
  exit 1
fi

if [[ "${ONESHIM_DEV_BUNDLE_SKIP_BUILD:-0}" != "1" ]]; then
  (
    cd "$ROOT_DIR/src-tauri"
    cargo tauri build --debug --config tauri.dev.conf.json --bundles app --ci
  )
fi

if [[ ! -d "$APP_PATH" ]]; then
  echo "error: expected app bundle not found: $APP_PATH" >&2
  exit 1
fi

codesign --force --deep --sign "$SIGN_IDENTITY" \
  --entitlements "$ENTITLEMENTS" \
  "$APP_PATH"

codesign --verify --deep --strict --verbose=2 "$APP_PATH"

BUNDLE_ID="$(/usr/libexec/PlistBuddy -c "Print CFBundleIdentifier" "$APP_PATH/Contents/Info.plist")"
DISPLAY_NAME="$(/usr/libexec/PlistBuddy -c "Print CFBundleDisplayName" "$APP_PATH/Contents/Info.plist")"

if [[ "$BUNDLE_ID" != "com.oneshim.client.dev" ]]; then
  echo "error: expected dev bundle identifier com.oneshim.client.dev, got: $BUNDLE_ID" >&2
  exit 1
fi

if [[ "$DISPLAY_NAME" != "Maekon Dev" ]]; then
  echo "error: expected dev display name Maekon Dev, got: $DISPLAY_NAME" >&2
  exit 1
fi

echo "Built and signed: $APP_PATH"
echo "Bundle identifier: $BUNDLE_ID"
echo "Display name: $DISPLAY_NAME"
echo "Launch for native QC: open -n \"$APP_PATH\""
echo "QC note: quit any installed release Maekon app before launch so macOS does not surface the release identity."

if [[ "$SIGN_IDENTITY" == "-" ]]; then
  echo "warning: ad-hoc signing uses a cdhash-based requirement; macOS TCC permissions may need to be granted again after rebuilds." >&2
  echo "warning: set ONESHIM_DEV_CODESIGN_IDENTITY to a local signing identity for stable Accessibility/Screen Recording permissions." >&2
fi
