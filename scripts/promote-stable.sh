#!/usr/bin/env bash
# promote-stable.sh — Promote a validated RC to a stable release.
#
# Usage:
#   ./scripts/promote-stable.sh 0.3.7-rc.1

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

RC_VERSION="$1"
if ! is_rc_version "${RC_VERSION}"; then
  die "promote-stable.sh는 RC 버전만 입력받습니다 (예: 0.3.7-rc.1)"
fi

STABLE_VERSION="$(base_version "${RC_VERSION}")"
RC_TAG="v${RC_VERSION}"
STABLE_TAG="v${STABLE_VERSION}"

cd "${REPO_ROOT}"
info "Stable 승격 준비: ${RC_TAG} -> ${STABLE_TAG}"

if ! require_main_branch; then
  CURRENT_BRANCH="$(git rev-parse --abbrev-ref HEAD)"
  die "main 브랜치에서만 stable 승격을 할 수 있습니다 (현재: ${CURRENT_BRANCH})"
fi
success "브랜치 확인: main"

if ! require_clean_worktree; then
  die "커밋되지 않았거나 스테이징된 변경사항이 있습니다. 먼저 정리하세요."
fi
success "작업 디렉터리 클린 상태 확인"

if ! git rev-parse "${RC_TAG}" >/dev/null 2>&1; then
  die "RC 태그 '${RC_TAG}'가 존재하지 않습니다"
fi
if git rev-parse "${STABLE_TAG}" >/dev/null 2>&1; then
  die "Stable 태그 '${STABLE_TAG}'가 이미 존재합니다"
fi

if ! ensure_head_matches_tag "${RC_TAG}"; then
  RC_COMMIT="$(git rev-parse "${RC_TAG}^{commit}")"
  HEAD_COMMIT="$(git rev-parse HEAD)"
  die "HEAD가 ${RC_TAG} 커밋과 다릅니다 (HEAD=${HEAD_COMMIT}, RC=${RC_COMMIT}). 검증된 RC 커밋에서만 승격하세요."
fi
success "HEAD가 ${RC_TAG} 커밋과 일치합니다"

CURRENT_CARGO_VERSION="$(workspace_version)"
CURRENT_FRONTEND_VERSION="$(frontend_version)"
if [[ "${CURRENT_CARGO_VERSION}" != "${RC_VERSION}" ]]; then
  die "Cargo.toml 버전이 ${RC_VERSION}이 아닙니다 (현재: ${CURRENT_CARGO_VERSION})"
fi
if [[ "${CURRENT_FRONTEND_VERSION}" != "${RC_VERSION}" ]]; then
  die "frontend/package.json 버전이 ${RC_VERSION}이 아닙니다 (현재: ${CURRENT_FRONTEND_VERSION})"
fi
if ! changelog_has_entry "${RC_VERSION}"; then
  die "CHANGELOG.md에 [${RC_VERSION}] 섹션이 없습니다"
fi
if ! LOCK_MISMATCHES="$(workspace_lock_mismatches "${RC_VERSION}" 2>&1)"; then
  die "RC 기준 Cargo.lock 워크스페이스 버전이 맞지 않습니다:\n${LOCK_MISMATCHES}"
fi
success "RC 메타데이터 검증 완료"

info "버전 파일을 stable ${STABLE_VERSION}으로 승격합니다..."
set_workspace_version "${STABLE_VERSION}"
set_frontend_version "${STABLE_VERSION}"
copy_changelog_section "${RC_VERSION}" "${STABLE_VERSION}"
info "Cargo.lock를 stable ${STABLE_VERSION}으로 동기화합니다..."
sync_workspace_lockfile "${STABLE_VERSION}"

if [[ "$(workspace_version)" != "${STABLE_VERSION}" ]]; then
  die "Cargo.toml stable 승격에 실패했습니다"
fi
if [[ "$(frontend_version)" != "${STABLE_VERSION}" ]]; then
  die "frontend/package.json stable 승격에 실패했습니다"
fi
if ! changelog_has_entry "${STABLE_VERSION}"; then
  die "CHANGELOG.md에 [${STABLE_VERSION}] 섹션 추가에 실패했습니다"
fi
if ! changelog_section_body_matches "${STABLE_VERSION}" "${RC_VERSION}"; then
  die "Stable CHANGELOG 섹션이 RC 섹션과 일치하지 않습니다"
fi
if ! LOCK_MISMATCHES="$(workspace_lock_mismatches "${STABLE_VERSION}" 2>&1)"; then
  die "Stable 기준 Cargo.lock 워크스페이스 버전 동기화에 실패했습니다:\n${LOCK_MISMATCHES}"
fi
success "Stable 메타데이터 동기화 완료"

git add Cargo.toml Cargo.lock CHANGELOG.md crates/oneshim-web/frontend/package.json
git commit -m "chore(release): ${STABLE_TAG}"
success "승격 커밋 완료"

git tag -a "${STABLE_TAG}" -m "Release ${STABLE_TAG}"
success "Stable 태그 생성 완료: ${STABLE_TAG}"

if [[ "${PROMOTE_STABLE_NO_PUSH:-0}" == "1" ]]; then
  success "PROMOTE_STABLE_NO_PUSH=1 이므로 원격 푸시는 건너뜁니다"
else
  info "main 브랜치를 origin에 푸시합니다..."
  git push origin main
  success "main 브랜치 푸시 완료"

  info "태그 ${STABLE_TAG}를 origin에 푸시합니다..."
  git push origin "${STABLE_TAG}"
  success "태그 푸시 완료"
fi
echo ""
echo -e "${GREEN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${GREEN}  Stable 승격 완료: ${STABLE_TAG}${NC}"
echo -e "${GREEN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""
echo "  GitHub Releases:"
echo "  https://github.com/pseudotop/oneshim-client/releases/tag/${STABLE_TAG}"
echo ""
