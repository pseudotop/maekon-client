#!/usr/bin/env bash
# verify-bundle-capabilities.sh — 빌드 산출물이 어떤 능력으로 컴파일됐는지 산출물에서 직접 읽는다 (#9659)
#
# 왜 필요한가:
#   `server` 는 default feature 가 아니다(제품 불변식 — 계정 없이 전 기능 동작).
#   그래서 로그인 가능한 빌드와 로그인 없는 빌드가 **둘 다 정상적으로 성공**하고,
#   성공한 빌드 출력만으로는 둘을 구분할 수 없었다. #9659 에서
#   `build-macos-dev-bundle.sh` 가 `--features` 를 전달하지 않는다는 사실이
#   시연 준비 중에야 드러난 이유가 이것이다.
#
#   이 스크립트는 **빌드에 무엇을 요청했는지가 아니라 바이너리에 무엇이 들어갔는지**를
#   읽는다. `src-tauri/src/build_capabilities.rs` 가 `cfg(feature = "server")`
#   으로 둘 중 하나의 마커 문자열만 컴파일해 넣고(`#[used]` 앵커로 링커
#   dead-strip 을 통과), 여기서 그 문자열을 찾는다. 로그인 구현을 게이트하는
#   것과 **같은 cfg** 이므로 산출물의 자기보고가 된다.
#
# 사용:
#   ./scripts/verify-bundle-capabilities.sh <경로>
#   ./scripts/verify-bundle-capabilities.sh --expect "server" <경로>
#   ./scripts/verify-bundle-capabilities.sh --expect "" <경로>      # 로그인 없음을 단언
#
#   <경로> 는 macOS `.app` 번들 디렉터리이거나 실행 파일이다. `.app` 이면
#   `Contents/MacOS/` 안의 실행 파일들을 훑는다(번들 실행 파일 이름은 Tauri
#   productName 에 따라 달라지므로 고정 이름을 가정하지 않는다).
#
#   `--expect` 는 cargo feature 목록(쉼표 또는 공백 구분)을 받아 산출물과
#   대조한다. `grep` 결과와 목록이 어긋나면 실패한다. 인자를 주지 않으면
#   보고만 하고 판정하지 않는다.
#
# 종료 코드:
#   0 — 마커를 정확히 하나 찾았고, `--expect` 를 줬다면 그것과 일치
#   1 — 마커 부재/모호(둘 다 존재)/기대와 불일치, 또는 사용법 오류
set -euo pipefail

# `src-tauri/src/build_capabilities.rs` 의 리터럴과 **바이트 단위로 동일**해야 한다.
# 두 곳의 철자가 갈라지면 이 스크립트는 "마커 없음"으로 조용히 실패하는 대신
# 시끄럽게 실패하지만, 애초에 갈라지지 않도록
# `crates/maekon-lint/tests/bundle_capability_marker_gate.rs` 가 대조한다.
MARKER_SERVER_ON="maekon-build-capability:server=on"
MARKER_SERVER_OFF="maekon-build-capability:server=off"

usage() {
  echo "usage: $(basename "$0") [--expect <cargo-features>] <path-to-.app-or-binary>" >&2
}

EXPECT_FEATURES=""
EXPECT_GIVEN=0
TARGET=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --expect)
      if [[ $# -lt 2 ]]; then
        echo "error: --expect requires a value (use --expect '' for a default build)" >&2
        usage
        exit 1
      fi
      EXPECT_FEATURES="$2"
      EXPECT_GIVEN=1
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    -*)
      echo "error: unknown option: $1" >&2
      usage
      exit 1
      ;;
    *)
      if [[ -n "$TARGET" ]]; then
        echo "error: expected exactly one path, got a second: $1" >&2
        usage
        exit 1
      fi
      TARGET="$1"
      shift
      ;;
  esac
done

if [[ -z "$TARGET" ]]; then
  usage
  exit 1
fi

if [[ ! -e "$TARGET" ]]; then
  echo "error: path does not exist: $TARGET" >&2
  exit 1
fi

# 검사 대상 파일 목록 만들기. `.app` 이면 Contents/MacOS/* 를 전부 후보로 둔다.
CANDIDATES=""
if [[ -d "$TARGET" ]]; then
  MACOS_DIR="$TARGET/Contents/MacOS"
  if [[ ! -d "$MACOS_DIR" ]]; then
    echo "error: directory is not a macOS app bundle (no Contents/MacOS): $TARGET" >&2
    exit 1
  fi
  # `! -type d` 로 일반 파일과 심볼릭 링크를 함께 잡는다. 조건부 대입은 쓰지
  # 않는다 — 루프 본문의 마지막 명령이 거짓이면 `set -e` 가 스크립트를 끝낸다.
  while IFS= read -r entry; do
    CANDIDATES="$CANDIDATES$entry"$'\n'
  done < <(find "$MACOS_DIR" -maxdepth 1 ! -type d)
  if [[ -z "$CANDIDATES" ]]; then
    echo "error: no executable files under $MACOS_DIR" >&2
    exit 1
  fi
else
  CANDIDATES="$TARGET"$'\n'
fi

# 마커 스캔. 사이드카(sandbox-worker 등)는 마커를 갖지 않으므로, 후보 중
# 마커를 가진 파일이 정본이다. 서로 다른 답을 내는 파일이 둘 이상이면 모호로
# 보고 실패한다 — 조용히 하나를 고르면 그 자체가 #9659 와 같은 함정이 된다.
FOUND_ON=0
FOUND_OFF=0
MARKED_FILE=""

# 개행 구분으로만 순회한다. 번들 경로에 공백이 들어간다("Maekon Dev.app")는
# 것이 이 스크립트의 기본 입력이므로 단어 분리에 맡길 수 없다.
while IFS= read -r candidate; do
  [[ -n "$candidate" && -f "$candidate" ]] || continue
  hit=0
  if LC_ALL=C grep -a -q -F -e "$MARKER_SERVER_ON" "$candidate" 2>/dev/null; then
    FOUND_ON=1
    hit=1
  fi
  if LC_ALL=C grep -a -q -F -e "$MARKER_SERVER_OFF" "$candidate" 2>/dev/null; then
    FOUND_OFF=1
    hit=1
  fi
  if [[ "$hit" == "1" && -z "$MARKED_FILE" ]]; then
    MARKED_FILE="$candidate"
  fi
# here-string(파이프 아님)이라 루프가 현재 셸에서 돌고 FOUND_* 대입이 살아남는다.
done <<< "$CANDIDATES"

if [[ "$FOUND_ON" == "0" && "$FOUND_OFF" == "0" ]]; then
  echo "error: no maekon build-capability marker found in: $TARGET" >&2
  echo "hint: the marker is compiled in by src-tauri/src/build_capabilities.rs." >&2
  echo "hint: an artifact without it was not built from this source tree, or the" >&2
  echo "hint: marker module was removed — do not assume it is login-capable." >&2
  exit 1
fi

if [[ "$FOUND_ON" == "1" && "$FOUND_OFF" == "1" ]]; then
  echo "error: BOTH server=on and server=off markers are present in: $TARGET" >&2
  echo "hint: exactly one is selected by cfg(feature = \"server\"); seeing both means" >&2
  echo "hint: the marker no longer tracks the feature and cannot be trusted." >&2
  exit 1
fi

if [[ "$FOUND_ON" == "1" ]]; then
  ACTUAL_SERVER="on"
else
  ACTUAL_SERVER="off"
fi

echo "Artifact: $TARGET"
echo "Inspected: ${MARKED_FILE:-$TARGET}"
echo "Capability: server=$ACTUAL_SERVER"
if [[ "$ACTUAL_SERVER" == "on" ]]; then
  echo "Login: AVAILABLE — Sign in (/login) and Settings > General > Account render the form."
else
  echo "Login: NOT COMPILED IN — the Sign in screen states connected mode is not in this build."
  echo "       This is the DEFAULT build and a supported configuration: no account is required"
  echo "       and no feature is withheld. Rebuild with the server feature if you need sign-in."
fi

if [[ "$EXPECT_GIVEN" == "0" ]]; then
  exit 0
fi

# 기대치 계산. `grpc` 는 Cargo.toml 에서 `grpc = ["server", ...]` 이므로 server 를
# 함의한다. 이 함의가 Cargo.toml 과 갈라지지 않도록
# `bundle_capability_marker_gate.rs` 가 Cargo.toml 쪽을 함께 검사한다.
EXPECTED_SERVER="off"
for token in ${EXPECT_FEATURES//,/ }; do
  case "$token" in
    server|grpc) EXPECTED_SERVER="on" ;;
    *) ;;
  esac
done

echo "Expected: server=$EXPECTED_SERVER (from features: '${EXPECT_FEATURES}')"

if [[ "$ACTUAL_SERVER" != "$EXPECTED_SERVER" ]]; then
  echo >&2
  echo "error: capability mismatch — requested features imply server=$EXPECTED_SERVER but the" >&2
  echo "       artifact reports server=$ACTUAL_SERVER." >&2
  if [[ "$EXPECTED_SERVER" == "on" ]]; then
    echo "cause: the feature list was almost certainly not forwarded to cargo. This is exactly" >&2
    echo "       the #9659 defect: the build succeeds and produces a binary that cannot sign in." >&2
  else
    echo "cause: the artifact carries the server feature although it was not requested — a build" >&2
    echo "       that opts a user into the account path without being asked to." >&2
  fi
  exit 1
fi

echo "OK: artifact capabilities match the requested feature set."
