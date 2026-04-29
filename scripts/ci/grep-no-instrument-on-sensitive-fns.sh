#!/usr/bin/env bash
# D13-v2b PR-B2 — first-line heuristic forbidding `#[tracing::instrument]`
# (or renamed `#[trace]`) on functions that handle the integration_auth_token
# or GrpcSpawnConfig. Auto-attribute-logging of those args would leak the
# token value to logs.
#
# This is a HEURISTIC, not an enforcement guarantee. It's bypassable via:
#  - multi-line attribute syntax `#[instrument(\n  skip_all\n)]`
#  - renamed re-export: `use tracing::instrument as trace; #[trace]`
#  - a contributor adding `#[instrument]` on a NEW sensitive fn not in this
#    allowlist
#
# The ACTUAL invariant guard is the `GrpcSpawnConfig::Debug` redaction test
# in `crates/oneshim-web/src/grpc/spawn_config.rs`. See spec IMP-V2-B + §6.
#
# Run locally: bash scripts/ci/grep-no-instrument-on-sensitive-fns.sh
# Exit 0 = no heuristic violation.
# Exit 1 = violation found, see stdout.

set -euo pipefail

SENSITIVE_FNS=(
    "fn subscribe_metrics"
    "fn honor_opt_out"
    "fn validate_authority"
    "fn serve"
    "fn serve_optional"
    "fn from_spawn_config"
)

violations=0
for fn in "${SENSITIVE_FNS[@]}"; do
    # -B3 captures preceding lines where attributes typically live. We then
    # filter for `#[...instrument` or `#[...trace` within that window. `rg`
    # is used because the workspace already uses it (it's listed alongside
    # lefthook/cargo in other hooks).
    if rg --line-number -B3 "$fn" crates/oneshim-web/src/grpc/ 2>/dev/null \
        | grep -E '^[[:space:]]*#\[[^]]*(instrument|trace)' \
        >/dev/null; then
        echo "❌ heuristic violation: attribute 'instrument' or 'trace' near '$fn'"
        violations=$((violations+1))
    fi
done

if [ "$violations" -gt 0 ]; then
    echo ""
    echo "See the internal sensitive-instrumentation spec for the allowlist rationale"
    echo "(This is a first-line heuristic; the invariant guard is the"
    echo " GrpcSpawnConfig Debug redaction test.)"
    exit 1
fi

echo "✓ no #[instrument] / #[trace] near sensitive fns (heuristic pass)"
