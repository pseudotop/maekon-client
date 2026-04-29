#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT_DIR"

MANIFEST_PATH="docs/contracts/http-interface-manifest.v1.json"
OUTPUT_PATH="${1:-docs/contracts/oneshim-web.v1.openapi.yaml}"

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

mkdir -p "$(dirname "$OUTPUT_PATH")"

cat > "$OUTPUT_PATH" <<EOF
openapi: 3.0.3
info:
  title: Maekon Local Web API
  version: "v1"
  description: |
    Auto-generated from docs/contracts/http-interface-manifest.v1.json.
    This snapshot is transport-level and contract-focused.
  x-oneshim-document-version: ${document_version}
  x-oneshim-updated-at: "${updated_at}"
  x-oneshim-routes-file: "${routes_file}"
  x-oneshim-contracts-crate: "${contracts_crate}"
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

for api_path in "${api_paths[@]}"; do
  printf '  "%s":\n' "$api_path" >> "$OUTPUT_PATH"

  while IFS=$'\t' read -r module method raw_path; do
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
      cat >> "$OUTPUT_PATH" <<'EOF'
      requestBody:
        required: false
        content:
          application/json:
            schema:
              $ref: '#/components/schemas/GenericObject'
EOF
    fi

    cat >> "$OUTPUT_PATH" <<'EOF'
      responses:
        "200":
          description: Success
          content:
            application/json:
              schema:
                $ref: '#/components/schemas/GenericObject'
        "default":
          description: Error
          content:
            application/json:
              schema:
                $ref: '#/components/schemas/ErrorResponse'
EOF
  done < <(
    jq -r --arg api_path "$api_path" '
      .groups[] as $g
      | $g.operations[]
      | {
          module: $g.module,
          method: (.method | ascii_downcase),
          raw_path: .path,
          normalized_path: (.path | gsub(":(?<p>[A-Za-z_][A-Za-z0-9_]*)"; "{\(.p)}"))
        }
      | select(.normalized_path == $api_path)
      | [.module, .method, .raw_path]
      | @tsv
    ' "$MANIFEST_PATH"
  )
done

cat >> "$OUTPUT_PATH" <<'EOF'
components:
  schemas:
    GenericObject:
      type: object
      additionalProperties: true
    ErrorResponse:
      type: object
      additionalProperties: true
EOF

echo "[http-openapi] generated: $OUTPUT_PATH"
