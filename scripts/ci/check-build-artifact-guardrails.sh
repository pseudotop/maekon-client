#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CLIENT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
CI_WORKFLOW="$CLIENT_ROOT/.github/workflows/ci.yml"

die() {
    echo "ci build artifact guardrail failure: $*" >&2
    exit 1
}

job_section() {
    local job_name="$1"
    awk -v job="  ${job_name}:" '
        $0 == job { in_job = 1 }
        /^  [A-Za-z0-9_-]+:/ && $0 != job && in_job { exit }
        in_job { print }
    ' "$CI_WORKFLOW"
}

if [ ! -f "$CI_WORKFLOW" ]; then
    die "missing CI workflow: $CI_WORKFLOW"
fi

if grep -R -q "echo '<html></html>' > crates/maekon-web/frontend/dist/index.html" "$CLIENT_ROOT/.github/workflows"; then
    die "workflow frontend dist stubs must not use blank <html></html>"
fi

if grep -R -q "echo '<!doctype html>' > crates/maekon-web/frontend/dist/index.html" "$CLIENT_ROOT/.github/workflows"; then
    die "workflow frontend dist stubs must include the Maekon dashboard shell text"
fi

frontend_section="$(job_section frontend)"
check_section="$(job_section check)"
build_section="$(job_section build)"

if [ -z "$frontend_section" ]; then
    die "frontend job section not found"
fi

if [ -z "$check_section" ]; then
    die "check job section not found"
fi

if [ -z "$build_section" ]; then
    die "build job section not found"
fi

grep -q "github.event_name != 'pull_request'" <<< "$frontend_section" \
    || die "frontend job must scope rust-only artifact support to non-PR builds"

grep -q "needs.changes.outputs.rust == 'true'" <<< "$frontend_section" \
    || die "frontend job must run for non-PR rust changes so build artifacts use real frontend dist"

grep -q "bash scripts/ci/check-build-artifact-guardrails.sh" <<< "$check_section" \
    || die "check job must run this guardrail script"

grep -q "needs: \\[changes, test, frontend\\]" <<< "$build_section" \
    || die "build job must depend on frontend before packaging app artifacts"

if grep -q "continue-on-error: true" <<< "$build_section"; then
    die "frontend artifact download in build job must fail closed"
fi

if grep -q "echo '<html></html>' > crates/maekon-web/frontend/dist/index.html" <<< "$build_section"; then
    die "build job must not create placeholder frontend dist/index.html"
fi

grep -q "Missing frontend-dist-bundle artifact; CI build artifacts require real frontend dist" <<< "$build_section" \
    || die "build job must emit an explicit missing frontend artifact error"

grep -q "Blank frontend dist stub detected in CI build artifact path" <<< "$build_section" \
    || die "build job must reject blank frontend dist stubs"

echo "ci build artifact guardrail checks passed"
