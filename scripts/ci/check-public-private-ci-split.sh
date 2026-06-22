#!/usr/bin/env bash
set -euo pipefail

case ":$PATH:" in
  *":/usr/bin:"*) ;;
  *) export PATH="/usr/bin:$PATH" ;;
esac

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CLIENT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
WORKFLOW_DIR="$CLIENT_ROOT/.github/workflows"
CI_WORKFLOW="$WORKFLOW_DIR/ci.yml"

die() {
  echo "public/private CI split guardrail failure: $*" >&2
  exit 1
}

has_pull_request_trigger() {
  grep -Eq '^[[:space:]]+pull_request:[[:space:]]*$' "$1"
}

require_file() {
  local path="$1"
  [[ -f "$path" ]] || die "missing required file: $path"
}

require_file "$CI_WORKFLOW"

shopt -s nullglob
workflow_files=("$WORKFLOW_DIR"/*.yml "$WORKFLOW_DIR"/*.yaml)
[[ ${#workflow_files[@]} -gt 0 ]] || die "no public workflow files found"

for workflow in "${workflow_files[@]}"; do
  if grep -Eq '^[[:space:]]+pull_request_target:[[:space:]]*$' "$workflow"; then
    die "$(basename "$workflow") must not use pull_request_target"
  fi

  if has_pull_request_trigger "$workflow"; then
    if grep -Fq 'secrets.' "$workflow"; then
      die "$(basename "$workflow") must not reference secrets from a pull_request workflow"
    fi

    if grep -Eq '^[[:space:]]+[A-Za-z0-9_-]+:[[:space:]]+write([[:space:]]|$)' "$workflow"; then
      die "$(basename "$workflow") pull_request workflow must not request write permissions"
    fi

    if ! grep -Eq '^permissions:[[:space:]]*$' "$workflow"; then
      die "$(basename "$workflow") pull_request workflow must declare top-level read-only permissions"
    fi
  fi
done

grep -Fq 'bash scripts/ci/check-public-private-ci-split.sh' "$CI_WORKFLOW" \
  || die "ci.yml must run check-public-private-ci-split.sh in the public check job"

echo "public/private CI split guardrail checks passed"
