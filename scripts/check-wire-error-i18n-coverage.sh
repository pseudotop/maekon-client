#!/usr/bin/env bash
# check-wire-error-i18n-coverage.sh — ADR-019 Follow-up #3 build-time guard.
#
# Ensures that every wire code listed in
#   crates/oneshim-core/tests/wire_contract_snapshot.expected.txt
# has a translation entry in BOTH
#   crates/oneshim-web/frontend/src/i18n/wire-errors.en.json
#   crates/oneshim-web/frontend/src/i18n/wire-errors.ko.json
#
# The Vitest suite `src/i18n/__tests__/translateError.test.ts` already
# enforces this at test time; this script exists so CI can fail fast
# before running the full test suite when a new wire code is added
# without its translations.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
REGISTRY="${ROOT_DIR}/crates/oneshim-core/tests/wire_contract_snapshot.expected.txt"
EN_FILE="${ROOT_DIR}/crates/oneshim-web/frontend/src/i18n/wire-errors.en.json"
KO_FILE="${ROOT_DIR}/crates/oneshim-web/frontend/src/i18n/wire-errors.ko.json"

if [[ ! -f "$REGISTRY" ]]; then
  echo "[FAIL] wire-contract snapshot missing: $REGISTRY" >&2
  exit 2
fi

for f in "$EN_FILE" "$KO_FILE"; do
  if [[ ! -f "$f" ]]; then
    echo "[FAIL] translation file missing: $f" >&2
    exit 2
  fi
done

declare -a MISSING_EN=()
declare -a MISSING_KO=()

while IFS= read -r code; do
  [[ -z "$code" ]] && continue
  # Use jq-free grep for "code": literal-match. The JSON file has one
  # "key": "value" per line (enforced by the existing `cargo fmt` style
  # the Rust side maintains; not a strict requirement, since jq would
  # be more robust but adds a dep).
  if ! grep -qF "\"$code\":" "$EN_FILE"; then
    MISSING_EN+=("$code")
  fi
  if ! grep -qF "\"$code\":" "$KO_FILE"; then
    MISSING_KO+=("$code")
  fi
done < "$REGISTRY"

FAIL=0
if (( ${#MISSING_EN[@]} > 0 )); then
  echo "[FAIL] ${#MISSING_EN[@]} wire code(s) missing from en translation:" >&2
  printf '  - %s\n' "${MISSING_EN[@]}" >&2
  FAIL=1
fi
if (( ${#MISSING_KO[@]} > 0 )); then
  echo "[FAIL] ${#MISSING_KO[@]} wire code(s) missing from ko translation:" >&2
  printf '  - %s\n' "${MISSING_KO[@]}" >&2
  FAIL=1
fi

if (( FAIL == 0 )); then
  COUNT=$(grep -c '^[[:space:]]*"[a-z]' "$EN_FILE" || true)
  echo "[OK] All wire codes have en+ko translations (${COUNT} keys per locale)."
fi

exit $FAIL
