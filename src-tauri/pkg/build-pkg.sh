#!/usr/bin/env bash
# build-pkg.sh — Build a macOS PKG installer with welcome, license, and conclusion screens.
#
# Usage:
#   ./src-tauri/pkg/build-pkg.sh [--sign "Developer ID Installer: ..."]
#
# Prerequisites:
#   - The Tauri app bundle must already exist at target/release/bundle/macos/Maekon.app
#   - Xcode command line tools (pkgbuild, productbuild)
#
# This script wraps Apple's productbuild to produce a professional installer
# with: Welcome -> License -> Install -> Conclusion flow.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
PKG_RESOURCES="$SCRIPT_DIR"
APP_BUNDLE="$PROJECT_ROOT/target/release/bundle/macos/Maekon.app"
OUTPUT_DIR="$PROJECT_ROOT/target/release/bundle/macos"
VERSION=$(grep -m1 'version' "$PROJECT_ROOT/Cargo.toml" | sed 's/.*"\(.*\)".*/\1/')

SIGN_IDENTITY=""
if [[ "${1:-}" == "--sign" && -n "${2:-}" ]]; then
  SIGN_IDENTITY="$2"
fi

echo "=== Maekon PKG Builder ==="
echo "Version: $VERSION"
echo "App Bundle: $APP_BUNDLE"

# Verify app bundle exists
if [[ ! -d "$APP_BUNDLE" ]]; then
  echo "ERROR: App bundle not found at $APP_BUNDLE"
  echo "Run 'cargo tauri build' first."
  exit 1
fi

# Create temp working directory
WORK_DIR=$(mktemp -d)
trap 'rm -rf "$WORK_DIR"' EXIT

# Step 1: Create component package
echo "Building component package..."
pkgbuild \
  --root "$APP_BUNDLE" \
  --install-location "/Applications/Maekon.app" \
  --identifier "com.oneshim.client" \
  --version "$VERSION" \
  "$WORK_DIR/oneshim-component.pkg"

# Step 2: Create distribution XML
cat > "$WORK_DIR/distribution.xml" << DISTEOF
<?xml version="1.0" encoding="utf-8" standalone="no"?>
<installer-gui-script minSpecVersion="2">
    <title>Maekon</title>
    <organization>com.oneshim</organization>
    <welcome file="welcome.html" mime-type="text/html" />
    <license file="license.html" mime-type="text/html" />
    <conclusion file="conclusion.html" mime-type="text/html" />
    <options customize="never" require-scripts="false" hostArchitectures="x86_64,arm64" />
    <domains enable_anywhere="false" enable_currentUserHome="false" enable_localSystem="true" />
    <choices-outline>
        <line choice="default">
            <line choice="com.oneshim.client" />
        </line>
    </choices-outline>
    <choice id="default" />
    <choice id="com.oneshim.client" visible="false">
        <pkg-ref id="com.oneshim.client" />
    </choice>
    <pkg-ref id="com.oneshim.client" version="$VERSION" onConclusion="none">#oneshim-component.pkg</pkg-ref>
</installer-gui-script>
DISTEOF

# Step 3: Copy resources
cp "$PKG_RESOURCES/welcome.html" "$WORK_DIR/"
cp "$PKG_RESOURCES/license.html" "$WORK_DIR/"
cp "$PKG_RESOURCES/conclusion.html" "$WORK_DIR/"

build_product_archive() {
  local output_pkg="$1"

  if [[ -n "$SIGN_IDENTITY" ]]; then
    echo "Signing with: $SIGN_IDENTITY"
    productbuild \
      --distribution "$WORK_DIR/distribution.xml" \
      --resources "$WORK_DIR" \
      --package-path "$WORK_DIR" \
      --sign "$SIGN_IDENTITY" \
      "$output_pkg"
  else
    productbuild \
      --distribution "$WORK_DIR/distribution.xml" \
      --resources "$WORK_DIR" \
      --package-path "$WORK_DIR" \
      "$output_pkg"
  fi
}

# Step 4: Build product archive
OUTPUT_PKG="$OUTPUT_DIR/Maekon-${VERSION}.pkg"
echo "Building product archive..."
build_product_archive "$OUTPUT_PKG"

echo ""
echo "=== PKG installer created ==="
echo "Output: $OUTPUT_PKG"
echo "Size: $(du -h "$OUTPUT_PKG" | cut -f1)"
