#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT_DIR"

OPENAPI_PATH="docs/contracts/maekon-web.v1.openapi.yaml"
TMP_OPENAPI="$(mktemp)"
trap 'rm -f "$TMP_OPENAPI"' EXIT

if [[ ! -f "$OPENAPI_PATH" ]]; then
  echo "[http-openapi] missing file: $OPENAPI_PATH" >&2
  echo "[http-openapi] run ./scripts/generate-http-openapi.sh to bootstrap it" >&2
  exit 1
fi

./scripts/generate-http-openapi.sh "$TMP_OPENAPI" >/dev/null

if ! diff -u "$OPENAPI_PATH" "$TMP_OPENAPI"; then
  echo "[http-openapi] snapshot drift detected: $OPENAPI_PATH" >&2
  echo "[http-openapi] run ./scripts/generate-http-openapi.sh and commit updated snapshot" >&2
  exit 1
fi

# ---------------------------------------------------------------------------
# Schema-body invariants (E20-17 #4809): the snapshot now carries real
# schemars-generated DTO body schemas, not just path stubs. Guard the
# load-bearing properties so a regression (e.g. forgetting to splice schemas,
# or a stray `$defs` ref) is caught locally even though the GitHub Actions
# diff gate is billing-disabled.
# ---------------------------------------------------------------------------

# 1) No leftover schemars-internal `$defs` refs (must be rewritten to
#    `#/components/schemas/...`).
if grep -q '#/\$defs/' "$OPENAPI_PATH"; then
  echo "[http-openapi] leftover '#/\$defs/' refs found — ref rewriting is broken" >&2
  exit 1
fi

# 2) components.schemas must carry far more than the GenericObject fallback
#    (full DTO disclosure). Use a conservative floor so the check is stable.
schema_count="$(
  awk '/^components:/{c=1} c&&/^    [A-Za-z0-9_]+:/{n++} END{print n+0}' "$OPENAPI_PATH"
)"
if [[ "$schema_count" -lt 100 ]]; then
  echo "[http-openapi] only $schema_count component schemas — expected the full DTO set" >&2
  echo "[http-openapi] the schema-emit splice likely failed (check the 'schema' feature)" >&2
  exit 1
fi

# 3) No `maekon-core` cross-crate type may surface as a named component schema.
#    Containment renders those as inline opaque objects; a named def here means
#    a `#[cfg_attr(... schemars(schema_with = ...))]` site was missed.
leaked="$(
  grep -nE "^    (WorkflowPreset|IntentResult|AutomationIntent|ElementBounds|UserOverrideAction|IntegrationAckCursor|IntegrationAuthStatus|IntegrationRuntimeTelemetry|IntegrationDeviceAuthorizationFlow|GuiActionRequest|GuiExecutionTicket|GuiInteractionSession|GuiBenchmarkDecision|GuiReadinessSnapshot|Attachment|ToolDefinition|MessageContext):" "$OPENAPI_PATH" || true
)"
if [[ -n "$leaked" ]]; then
  echo "[http-openapi] maekon-core type leaked into components.schemas (containment gap):" >&2
  echo "$leaked" >&2
  exit 1
fi

echo "[http-openapi] snapshot is up to date ($schema_count component schemas)"
