#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
CARGO_CMD="$ROOT_DIR/scripts/cargo-cache.sh"

cd "$ROOT_DIR"

if [[ $# -gt 0 ]]; then
  case "$1" in
    non-english|i18n|all)
      "$CARGO_CMD" run -p oneshim-lint --bin language-check -- "$@"
      exit $?
      ;;
  esac
fi

"$CARGO_CMD" run -p oneshim-lint --bin language-check -- all "$@"
