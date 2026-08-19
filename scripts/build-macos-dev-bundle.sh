#!/usr/bin/env bash
# build-macos-dev-bundle.sh — macOS 디버그 `.app` 번들 빌드 + 서명 + 능력 검증
#
# 기본값은 **default feature 빌드**다. `server` 는 default 가 아니므로 이 경로로
# 만든 번들은 로그인이 컴파일되지 않는다 — 그것이 설계 의도다(계정 없이 전
# 기능 동작). 문제였던 것은 로그인 가능한 번들을 만들 방법이 **없었다**는 점과,
# 성공한 빌드 출력만으로 둘을 구분할 수 없었다는 점이다(#9659).
#
# 로그인 가능한 번들(시연·연동 QC용):
#   MAEKON_DEV_BUNDLE_FEATURES=server ./scripts/build-macos-dev-bundle.sh
#
# 환경변수:
#   MAEKON_DEV_BUNDLE_FEATURES   cargo feature 목록(쉼표 구분). 비면 default 빌드.
#                                `server` = 로그인/서버 전송. `grpc` 는 server 를 함의.
#   MAEKON_DEV_BUNDLE_SKIP_BUILD 1 이면 빌드를 건너뛰고 기존 산출물을 서명·검증만 한다.
#   MAEKON_DEV_CODESIGN_IDENTITY 로컬 서명 아이덴티티(미지정 시 ad-hoc `-`).
#
# 산출물은 마지막에 `verify-bundle-capabilities.sh` 로 **바이너리를 직접 읽어**
# 능력을 보고한다. 요청한 feature 와 산출물이 어긋나면 실패한다 — 이 스크립트가
# `--features` 전달을 잃어버리면 조용히 다른 물건을 내놓는 대신 여기서 멈춘다.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
APP_PATH="$ROOT_DIR/target/debug/bundle/macos/Maekon Dev.app"
ENTITLEMENTS="$ROOT_DIR/src-tauri/assets/maekon.entitlements"
SIGN_IDENTITY="${MAEKON_DEV_CODESIGN_IDENTITY:--}"
# 공백을 쉼표로 정규화해 cargo 가 받는 단일 feature 목록 토큰으로 만든다.
BUNDLE_FEATURES="$(printf '%s' "${MAEKON_DEV_BUNDLE_FEATURES:-}" | tr -s ' ,' ',,' | sed 's/^,//; s/,$//')"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "error: macOS dev bundle signing is only available on Darwin." >&2
  exit 1
fi

if [[ "${MAEKON_DEV_BUNDLE_SKIP_BUILD:-0}" != "1" ]]; then
  (
    cd "$ROOT_DIR/src-tauri"
    # `--features` 는 요청이 있을 때만 붙인다. 인자를 붙이지 않는 경로가 곧
    # default(로그인 없음) 빌드이므로, 옵트인은 구조적으로 보장된다.
    if [[ -n "$BUNDLE_FEATURES" ]]; then
      echo "Building with cargo features: $BUNDLE_FEATURES"
      cargo tauri build --debug --config tauri.dev.conf.json --bundles app --ci \
        --features "$BUNDLE_FEATURES"
    else
      echo "Building with default cargo features (no sign-in compiled in)."
      cargo tauri build --debug --config tauri.dev.conf.json --bundles app --ci
    fi
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
SIGNATURE_DETAILS="$(codesign -dv --verbose=4 "$APP_PATH" 2>&1 || true)"
CDHASH="$(printf '%s\n' "$SIGNATURE_DETAILS" | awk -F= '/^CDHash=/ { print $2; exit }')"
SIGNATURE_KIND="$(printf '%s\n' "$SIGNATURE_DETAILS" | awk -F= '/^Signature=/ { print $2; exit }')"

if [[ "$BUNDLE_ID" != "com.maekon.app.dev" ]]; then
  echo "error: expected dev bundle identifier com.maekon.app.dev, got: $BUNDLE_ID" >&2
  exit 1
fi

if [[ "$DISPLAY_NAME" != "Maekon Dev" ]]; then
  echo "error: expected dev display name Maekon Dev, got: $DISPLAY_NAME" >&2
  exit 1
fi

echo "Built and signed: $APP_PATH"
echo "Bundle identifier: $BUNDLE_ID"
echo "Display name: $DISPLAY_NAME"
echo "Code signature: ${SIGNATURE_KIND:-unknown}"
echo "CDHash: ${CDHASH:-unknown}"

# 산출물 자기보고 (#9659). 기대치는 위의 cargo 호출을 구동한 것과 **같은**
# `$BUNDLE_FEATURES` 에서 유도된다. 누군가 위에서 `--features` 전달을 지우면
# 기대치는 그대로 server=on 인데 바이너리는 off 가 되어 여기서 실패한다.
if ! "$SCRIPT_DIR/verify-bundle-capabilities.sh" --expect "$BUNDLE_FEATURES" "$APP_PATH"; then
  if [[ "${MAEKON_DEV_BUNDLE_SKIP_BUILD:-0}" == "1" ]]; then
    echo "hint: MAEKON_DEV_BUNDLE_SKIP_BUILD=1 — the inspected bundle is a PREVIOUS build," >&2
    echo "hint: not one produced from the feature set requested just now." >&2
  fi
  exit 1
fi

echo "Re-check this artifact any time: ./scripts/verify-bundle-capabilities.sh \"$APP_PATH\""
echo "Launch for native QC: open -n \"$APP_PATH\""
echo "QC note: quit any installed release Maekon app before launch so macOS does not surface the release identity."
echo "TCC diagnostic: ./scripts/diagnose-macos-dev-tcc.sh"

if [[ "$SIGN_IDENTITY" == "-" ]]; then
  echo "warning: ad-hoc signing uses a cdhash-based requirement; macOS TCC permissions may need to be granted again after rebuilds." >&2
  echo "warning: System Settings can show Maekon Dev enabled for an older cdhash while the rebuilt app still probes permissions as missing." >&2
  echo "warning: set MAEKON_DEV_CODESIGN_IDENTITY to a local signing identity for stable Accessibility/Screen Recording permissions." >&2
fi
