#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "${ROOT_DIR}"

GENERATED_DIR="crates/oneshim-network/src/proto/generated"
HEAD_SHA="$(git rev-parse HEAD)"

resolve_range() {
  local base
  base=""

  if git rev-parse --verify origin/main >/dev/null 2>&1; then
    base="$(git merge-base HEAD origin/main)"
  elif git rev-parse --verify origin/develop >/dev/null 2>&1; then
    base="$(git merge-base HEAD origin/develop)"
  fi

  if [[ -n "${base}" && "${base}" != "${HEAD_SHA}" ]]; then
    printf '%s...HEAD\n' "${base}"
    return
  fi

  if git rev-parse --verify HEAD~1 >/dev/null 2>&1; then
    printf 'HEAD~1...HEAD\n'
    return
  fi

  printf '\n'
}

RANGE="$(resolve_range)"

if [[ -z "${RANGE}" ]]; then
  echo "[grpc-compat] Not enough git history to evaluate compatibility. Skipping."
  exit 0
fi

echo "[grpc-compat] Checking compatibility against range: ${RANGE}"

if [[ "${GRPC_COMPAT_ALLOW_BREAKING:-0}" == "1" ]]; then
  echo "[grpc-compat] GRPC_COMPAT_ALLOW_BREAKING=1 set; compatibility failures are bypassed."
  exit 0
fi

CHANGED_FILES="$(git diff --name-only "${RANGE}" -- "${GENERATED_DIR}")"
if [[ -z "${CHANGED_FILES}" ]]; then
  echo "[grpc-compat] No generated gRPC contract changes detected."
  exit 0
fi

echo "[grpc-compat] Generated contract changes detected:"
printf '%s\n' "${CHANGED_FILES}"

DIFF_CONTENT="$(git diff --unified=0 "${RANGE}" -- "${GENERATED_DIR}")"

BREAKING_LINES="$(printf '%s\n' "${DIFF_CONTENT}" | grep -E '^-\s*(pub (struct|enum|trait|mod)\s+|pub async fn\s+|#\[prost\(.*tag = ")' || true)"

if [[ -n "${BREAKING_LINES}" ]]; then
  echo "[grpc-compat] Potential breaking contract changes found:" >&2
  printf '%s\n' "${BREAKING_LINES}" >&2
  echo "[grpc-compat] Remove/breaking changes are blocked by default. Additive changes only for v1." >&2
  echo "[grpc-compat] If this is intentional for a major migration, run with GRPC_COMPAT_ALLOW_BREAKING=1 and document the migration." >&2
  exit 1
fi

echo "[grpc-compat] Compatibility gate passed (additive/non-breaking changes only)."
