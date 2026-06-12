#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT_DIR"

MANIFEST_PATH="docs/contracts/http-interface-manifest.v1.json"
OUTPUT_PATH="${1:-docs/contracts/maekon-web.v1.openapi.yaml}"

# `SCHEMAS_JSON` lets callers (verify scripts / CI) feed a precomputed
# schema-emit JSON instead of re-running `cargo`. When unset, we invoke the
# `emit-openapi-schemas` binary behind the `schema` cargo feature.
SCHEMAS_JSON="${SCHEMAS_JSON:-}"

readarray_compat() {
  local target="$1"
  if command -v mapfile >/dev/null 2>&1; then
    mapfile -t "$target"
    return
  fi

  eval "$target=()"
  local line
  while IFS= read -r line; do
    eval "$target+=(\"\$line\")"
  done
}

if ! command -v jq >/dev/null 2>&1; then
  echo "[http-openapi] jq is required" >&2
  exit 1
fi

if [[ ! -f "$MANIFEST_PATH" ]]; then
  echo "[http-openapi] missing manifest: $MANIFEST_PATH" >&2
  exit 1
fi

document_version="$(jq -r '.document_version' "$MANIFEST_PATH")"
updated_at="$(jq -r '.updated_at' "$MANIFEST_PATH")"
routes_file="$(jq -r '.source.routes_file' "$MANIFEST_PATH")"
contracts_crate="$(jq -r '.source.contracts_crate' "$MANIFEST_PATH")"

# ---------------------------------------------------------------------------
# Obtain the schemars-generated DTO schemas (full disclosure, owner decision).
# Cross-crate `maekon-core` field types are contained as opaque objects by the
# contract crate, so this JSON never recurses into `maekon-core`.
# ---------------------------------------------------------------------------
SCHEMAS_TMP="$(mktemp)"
trap 'rm -f "$SCHEMAS_TMP"' EXIT

if [[ -n "$SCHEMAS_JSON" ]]; then
  if [[ ! -f "$SCHEMAS_JSON" ]]; then
    echo "[http-openapi] SCHEMAS_JSON not found: $SCHEMAS_JSON" >&2
    exit 1
  fi
  cp "$SCHEMAS_JSON" "$SCHEMAS_TMP"
else
  if ! command -v cargo >/dev/null 2>&1; then
    echo "[http-openapi] cargo is required to emit DTO schemas (or set SCHEMAS_JSON)" >&2
    exit 1
  fi
  if ! cargo run --quiet -p maekon-api-contracts \
      --bin emit-openapi-schemas --features schema >"$SCHEMAS_TMP" 2>/dev/null; then
    echo "[http-openapi] failed to emit DTO schemas via cargo" >&2
    exit 1
  fi
fi

if ! jq -e 'type == "object"' "$SCHEMAS_TMP" >/dev/null 2>&1; then
  echo "[http-openapi] emitted schema JSON is not an object" >&2
  exit 1
fi

mkdir -p "$(dirname "$OUTPUT_PATH")"

cat > "$OUTPUT_PATH" <<EOF
openapi: 3.0.3
info:
  title: Maekon Local Web API
  version: "v1"
  description: |
    Auto-generated from docs/contracts/http-interface-manifest.v1.json.
    Request/response bodies are real schemars-generated DTO schemas
    (full disclosure; the API is loopback-only + token-gated). Cross-crate
    maekon-core field types are rendered as opaque objects (build containment).
  x-maekon-document-version: ${document_version}
  x-maekon-updated-at: "${updated_at}"
  x-maekon-routes-file: "${routes_file}"
  x-maekon-contracts-crate: "${contracts_crate}"
servers:
  - url: /
paths:
EOF

readarray_compat api_paths < <(
  jq -r '[.groups[].operations[].path | gsub(":(?<p>[A-Za-z_][A-Za-z0-9_]*)"; "{\(.p)}")] | unique[]' "$MANIFEST_PATH"
)

if [[ ${#api_paths[@]} -eq 0 ]]; then
  echo "[http-openapi] no paths discovered in manifest" >&2
  exit 1
fi

# Emits a `schema:` block (6-space indented `schema:` key) for a body, given a
# manifest type name + array flag. Falls back to GenericObject when unmapped.
emit_body_schema() {
  local type_name="$1"
  local is_array="$2"
  local indent="$3"

  if [[ -z "$type_name" || "$type_name" == "null" || "$type_name" == "-" ]]; then
    printf '%sschema:\n%s  $ref: '\''#/components/schemas/GenericObject'\''\n' "$indent" "$indent"
    return
  fi

  if [[ "$is_array" == "true" ]]; then
    printf '%sschema:\n' "$indent"
    printf '%s  type: array\n' "$indent"
    printf '%s  items:\n' "$indent"
    printf '%s    $ref: '\''#/components/schemas/%s'\''\n' "$indent" "$type_name"
  else
    printf '%sschema:\n%s  $ref: '\''#/components/schemas/%s'\''\n' "$indent" "$indent" "$type_name"
  fi
}

for api_path in "${api_paths[@]}"; do
  printf '  "%s":\n' "$api_path" >> "$OUTPUT_PATH"

  while IFS=$'\t' read -r module method raw_path request_type response_type request_is_array response_is_array; do
    operation_id="$(
      printf '%s_%s_%s' "$module" "$method" "$api_path" \
        | sed -E 's/[{}]//g; s/[^A-Za-z0-9]+/_/g; s/^_+//; s/_+$//'
    )"

    summary="$(printf '%s %s' "$(printf '%s' "$method" | tr '[:lower:]' '[:upper:]')" "$raw_path")"

    {
      printf '    %s:\n' "$method"
      printf '      tags:\n'
      printf '        - %s\n' "$module"
      printf '      operationId: %s\n' "$operation_id"
      printf '      summary: "%s"\n' "$summary"
    } >> "$OUTPUT_PATH"

    readarray_compat path_params < <(
      printf '%s\n' "$api_path" \
        | grep -oE '\{[A-Za-z_][A-Za-z0-9_]*\}' \
        | tr -d '{}' \
        | awk '!seen[$0]++' \
        || true
    )

    if [[ ${#path_params[@]} -gt 0 ]]; then
      {
        printf '      parameters:\n'
        for param in "${path_params[@]}"; do
          printf '        - name: %s\n' "$param"
          printf '          in: path\n'
          printf '          required: true\n'
          printf '          schema:\n'
          printf '            type: string\n'
        done
      } >> "$OUTPUT_PATH"
    fi

    if [[ "$method" =~ ^(post|put|delete)$ ]]; then
      {
        printf '      requestBody:\n'
        printf '        required: false\n'
        printf '        content:\n'
        printf '          application/json:\n'
        emit_body_schema "$request_type" "$request_is_array" '            '
      } >> "$OUTPUT_PATH"
    fi

    {
      printf '      responses:\n'
      printf '        "200":\n'
      printf '          description: Success\n'
      printf '          content:\n'
      printf '            application/json:\n'
      emit_body_schema "$response_type" "$response_is_array" '              '
      printf '        "default":\n'
      printf '          description: Error\n'
      printf '          content:\n'
      printf '            application/json:\n'
      printf '              schema:\n'
      printf '                $ref: '\''#/components/schemas/ErrorResponse'\''\n'
    } >> "$OUTPUT_PATH"
  done < <(
    jq -r --arg api_path "$api_path" '
      .groups[] as $g
      | $g.operations[]
      | {
          module: $g.module,
          method: (.method | ascii_downcase),
          raw_path: .path,
          normalized_path: (.path | gsub(":(?<p>[A-Za-z_][A-Za-z0-9_]*)"; "{\(.p)}")),
          request_type: (.request_type // "-"),
          response_type: (.response_type // "-"),
          request_is_array: (.request_is_array // false),
          response_is_array: (.response_is_array // false)
        }
      | select(.normalized_path == $api_path)
      # Use "-" sentinels for empty type columns: tab is whitespace, so bash
      # `read` with IFS=$'\t' collapses consecutive empty fields and shifts the
      # remaining columns. Non-empty sentinels keep the column alignment stable.
      | [.module, .method, .raw_path, .request_type, .response_type,
         (.request_is_array | tostring), (.response_is_array | tostring)]
      | @tsv
    ' "$MANIFEST_PATH"
  )
done

# ---------------------------------------------------------------------------
# components.schemas: a GenericObject fallback (for unmapped bodies) plus every
# emitted DTO schema. The contract crate emits its own real `ErrorResponse`
# (referenced by every operation's "default" response), so we do NOT add a
# hardcoded fallback for it — that would create a duplicate YAML key. The
# schemars `$defs` use `#/$defs/...` internal refs; rewrite them to
# `#/components/schemas/...` so they resolve inside the OpenAPI document.
# ---------------------------------------------------------------------------
if ! jq -e 'has("ErrorResponse")' "$SCHEMAS_TMP" >/dev/null; then
  echo "[http-openapi] emitted schemas missing ErrorResponse (expected from the contract crate)" >&2
  exit 1
fi

{
  printf 'components:\n'
  printf '  schemas:\n'
  printf '    GenericObject:\n'
  printf '      type: object\n'
  printf '      additionalProperties: true\n'
} >> "$OUTPUT_PATH"

# Rewrite internal refs, then render each schema as indented YAML (JSON is valid
# YAML, so we indent the compact JSON under each schema name). Sorted by key so
# the snapshot is deterministic across schemars/jq object-ordering changes.
jq -r '
  to_entries
  | sort_by(.key)[]
  | "    \(.key): " + (.value | tojson | gsub("#/\\$defs/"; "#/components/schemas/"))
' "$SCHEMAS_TMP" >> "$OUTPUT_PATH"

echo "[http-openapi] generated: $OUTPUT_PATH (schemas: $(jq 'keys | length' "$SCHEMAS_TMP"))"
