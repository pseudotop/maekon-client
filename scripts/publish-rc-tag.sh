#!/usr/bin/env bash
# publish-rc-tag.sh — Publish an RC tag from protected main after the PR is merged.
#
# Usage:
#   ./scripts/publish-rc-tag.sh 0.3.7-rc.1

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
  echo "사용법: $0 <rc-버전>" >&2
  echo "  예시: $0 0.3.7-rc.1" >&2
  exit 1
fi

VERSION="$1"
if ! is_rc_version "${VERSION}"; then
  die "publish-rc-tag.sh는 RC 버전만 허용합니다 (예: 0.3.7-rc.1)"
fi

TAG="v${VERSION}"

cd "${REPO_ROOT}"
info "RC 태그 발행 준비: ${TAG}"

if ! require_main_branch; then
  CURRENT_BRANCH="$(git rev-parse --abbrev-ref HEAD)"
  die "RC 태그는 main에서만 발행할 수 있습니다 (현재: ${CURRENT_BRANCH})"
fi
success "브랜치 확인: main"

if ! require_clean_worktree; then
  die "커밋되지 않았거나 스테이징된 변경사항이 있습니다. 먼저 정리하세요."
fi
success "작업 디렉터리 클린 상태 확인"

# Fetch latest main; --tags can warn about existing tags (non-fatal)
git fetch origin main --tags 2>&1 | grep -v "would clobber" || true
MAIN_SHA="$(git rev-parse origin/main)"
HEAD_SHA="$(git rev-parse HEAD)"
if [[ "${HEAD_SHA}" != "${MAIN_SHA}" ]]; then
  die "HEAD가 origin/main 최신 커밋이 아닙니다 (HEAD=${HEAD_SHA}, origin/main=${MAIN_SHA})"
fi
success "HEAD가 origin/main 최신 커밋과 일치합니다"

./scripts/pre-release-check.sh "${VERSION}"

if git rev-parse "${TAG}" >/dev/null 2>&1; then
  die "태그 '${TAG}'가 이미 존재합니다"
fi

info "태그를 생성합니다: ${TAG}"
git tag -a "${TAG}" -m "Release ${TAG}"
success "태그 생성 완료: ${TAG}"

info "태그 ${TAG}를 origin에 푸시합니다..."
git push origin "${TAG}"
success "태그 푸시 완료"

echo ""
echo -e "${GREEN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${GREEN}  RC 태그 발행 완료: ${TAG}${NC}"
echo -e "${GREEN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""
echo "  GitHub Releases (CI가 완료되면 자동 생성됩니다):"
echo "  https://github.com/pseudotop/maekon-client/releases/tag/${TAG}"
echo ""
