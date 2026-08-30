#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: scripts/verify-macos-app-bundle.sh [options] <Maekon.app>

Options:
  --expected-short-version <version>  Expected CFBundleShortVersionString.
  --expected-build-version <version>  Expected CFBundleVersion.
  --allow-adhoc                       Permit an ad-hoc signature (development only).
EOF
}

die() {
  echo "error: $*" >&2
  exit 1
}

EXPECTED_SHORT_VERSION=""
EXPECTED_BUILD_VERSION=""
ALLOW_ADHOC=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --expected-short-version)
      [[ $# -ge 2 ]] || die "$1 requires a value"
      EXPECTED_SHORT_VERSION="$2"
      shift 2
      ;;
    --expected-build-version)
      [[ $# -ge 2 ]] || die "$1 requires a value"
      EXPECTED_BUILD_VERSION="$2"
      shift 2
      ;;
    --allow-adhoc)
      ALLOW_ADHOC=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    --*)
      die "unknown option: $1"
      ;;
    *)
      break
      ;;
  esac
done

[[ $# -eq 1 ]] || {
  usage >&2
  exit 2
}
[[ "$(uname -s)" == "Darwin" ]] || die "this verifier must run on macOS"

APP_PATH="$1"
PLIST_PATH="$APP_PATH/Contents/Info.plist"
MAIN_BINARY="$APP_PATH/Contents/MacOS/maekon"
SIDECAR_BINARY="$APP_PATH/Contents/MacOS/maekon-sandbox-worker"

[[ -d "$APP_PATH" ]] || die "app bundle not found: $APP_PATH"
[[ -f "$PLIST_PATH" ]] || die "Info.plist not found: $PLIST_PATH"
[[ -x "$MAIN_BINARY" ]] || die "main executable not found: $MAIN_BINARY"
[[ -x "$SIDECAR_BINARY" ]] || die "sandbox worker not found: $SIDECAR_BINARY"

plutil -lint "$PLIST_PATH" >/dev/null
SHORT_VERSION="$(/usr/libexec/PlistBuddy -c 'Print CFBundleShortVersionString' "$PLIST_PATH")"
BUILD_VERSION="$(/usr/libexec/PlistBuddy -c 'Print CFBundleVersion' "$PLIST_PATH")"

if [[ -n "$EXPECTED_SHORT_VERSION" && "$SHORT_VERSION" != "$EXPECTED_SHORT_VERSION" ]]; then
  die "CFBundleShortVersionString is $SHORT_VERSION, expected $EXPECTED_SHORT_VERSION"
fi
if [[ -n "$EXPECTED_BUILD_VERSION" && "$BUILD_VERSION" != "$EXPECTED_BUILD_VERSION" ]]; then
  die "CFBundleVersion is $BUILD_VERSION, expected $EXPECTED_BUILD_VERSION"
fi
if [[ ! "$BUILD_VERSION" =~ ^[0-9]{1,4}(\.[0-9]{1,2}){0,2}((d|a|b|fc)([1-9]|[1-9][0-9]|1[0-9]{2}|2[0-4][0-9]|25[0-5]))?$ ]]; then
  die "CFBundleVersion is not an Apple-compatible build version: $BUILD_VERSION"
fi

VERIFY_OUTPUT="$(codesign --verify --deep --strict --verbose=4 "$APP_PATH" 2>&1)" || {
  printf '%s\n' "$VERIFY_OUTPUT" >&2
  die "app bundle code-sign verification failed"
}

SIGNATURE_DETAILS="$(codesign --display --verbose=5 "$APP_PATH" 2>&1)" || {
  printf '%s\n' "$SIGNATURE_DETAILS" >&2
  die "could not inspect app bundle signature"
}
if grep -Fq 'Info.plist=not bound' <<<"$SIGNATURE_DETAILS"; then
  printf '%s\n' "$SIGNATURE_DETAILS" >&2
  die "Info.plist is not bound into the app signature"
fi
grep -Eq '^Info\.plist entries=[1-9][0-9]*$' <<<"$SIGNATURE_DETAILS" \
  || die "signature metadata does not report a bound Info.plist"

if [[ "$ALLOW_ADHOC" == "1" ]]; then
  grep -Eq '^Signature=(adhoc|.*)$' <<<"$SIGNATURE_DETAILS" \
    || die "app bundle has no inspectable signature"
else
  grep -Fq 'Authority=Developer ID Application:' <<<"$SIGNATURE_DETAILS" \
    || die "app bundle is not signed by a Developer ID Application identity"
  grep -Eq '^TeamIdentifier=[A-Z0-9]+$' <<<"$SIGNATURE_DETAILS" \
    || die "app bundle signature has no TeamIdentifier"
fi

ENTITLEMENTS_OUTPUT="$(codesign --display --entitlements - "$APP_PATH" 2>&1)" || {
  printf '%s\n' "$ENTITLEMENTS_OUTPUT" >&2
  die "could not decode app entitlements"
}
if grep -Fiq 'invalid entitlements blob' <<<"$ENTITLEMENTS_OUTPUT"; then
  printf '%s\n' "$ENTITLEMENTS_OUTPUT" >&2
  die "app signature contains an invalid entitlements blob"
fi

for arch in $(lipo -archs "$MAIN_BINARY"); do
  ARCH_OUTPUT="$(codesign --verify --strict --verbose=4 --arch "$arch" "$APP_PATH" 2>&1)" || {
    printf '%s\n' "$ARCH_OUTPUT" >&2
    die "app signature is invalid for architecture $arch"
  }
done
codesign --verify --strict --verbose=4 "$SIDECAR_BINARY"

echo "macOS app bundle verified: $APP_PATH"
echo "  short version: $SHORT_VERSION"
echo "  build version: $BUILD_VERSION"
