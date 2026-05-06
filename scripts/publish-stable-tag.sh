#!/usr/bin/env bash
# publish-stable-tag.sh — Publish a signed stable tag from protected main.
#
# Usage:
#   ./scripts/publish-stable-tag.sh 0.3.7

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
source "${SCRIPT_DIR}/release-common.sh"

RED='\033[0;31m'
GREEN='\033[0;32m'
BLUE='\033[0;34m'
NC='\033[0m'

info()    { echo -e "${BLUE}[info]${NC}  $*"; }
success() { echo -e "${GREEN}[ok]${NC}    $*"; }
error()   { echo -e "${RED}[error]${NC} $*" >&2; }
die()     { error "$*"; exit 1; }

if [[ $# -ne 1 ]]; then
  echo "사용법: $0 <stable-버전>" >&2
  echo "  예시: $0 0.3.7" >&2
  exit 1
fi

VERSION="$1"
if ! is_stable_version "${VERSION}"; then
  die "publish-stable-tag.sh는 stable 버전만 허용합니다 (예: 0.3.7)"
fi

TAG="v${VERSION}"

cd "${REPO_ROOT}"
info "Stable 태그 발행 준비: ${TAG}"

if ! require_main_branch; then
  CURRENT_BRANCH="$(git rev-parse --abbrev-ref HEAD)"
  die "Stable 태그는 main에서만 발행할 수 있습니다 (현재: ${CURRENT_BRANCH})"
fi
success "브랜치 확인: main"

if ! require_clean_worktree; then
  die "커밋되지 않았거나 스테이징된 변경사항이 있습니다. 먼저 정리하세요."
fi
success "작업 디렉터리 클린 상태 확인"

git fetch origin main --tags 2>&1 | grep -v "would clobber" || true
MAIN_SHA="$(git rev-parse origin/main)"
HEAD_SHA="$(git rev-parse HEAD)"
if [[ "${HEAD_SHA}" != "${MAIN_SHA}" ]]; then
  die "HEAD가 origin/main 최신 커밋이 아닙니다 (HEAD=${HEAD_SHA}, origin/main=${MAIN_SHA})"
fi
success "HEAD가 origin/main 최신 커밋과 일치합니다"

if [[ "$(workspace_version)" != "${VERSION}" ]]; then
  die "Cargo.toml 버전이 ${VERSION}이 아닙니다 (현재: $(workspace_version))"
fi
if [[ "$(frontend_version)" != "${VERSION}" ]]; then
  die "frontend/package.json 버전이 ${VERSION}이 아닙니다 (현재: $(frontend_version))"
fi
if ! changelog_has_entry "${VERSION}"; then
  die "CHANGELOG.md에 [${VERSION}] 섹션이 없습니다"
fi
if ! LOCK_MISMATCHES="$(workspace_lock_mismatches "${VERSION}" 2>&1)"; then
  die "Cargo.lock 워크스페이스 버전이 ${VERSION}과 동기화되지 않았습니다:\n${LOCK_MISMATCHES}"
fi

./scripts/pre-release-check.sh "${VERSION}"

if git rev-parse "${TAG}" >/dev/null 2>&1; then
  die "태그 '${TAG}'가 이미 존재합니다"
fi

info "서명된 태그를 생성합니다: ${TAG}"
git tag -s "${TAG}" -m "Release ${TAG}"
success "태그 생성 완료: ${TAG}"

info "태그 ${TAG}를 origin에 푸시합니다..."
git push origin "${TAG}"
success "태그 푸시 완료"

echo ""
echo -e "${GREEN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${GREEN}  Stable 태그 발행 완료: ${TAG}${NC}"
echo -e "${GREEN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""
echo "  GitHub Releases (CI가 완료되면 자동 생성됩니다):"
echo "  https://github.com/pseudotop/maekon-client/releases/tag/${TAG}"
echo ""
