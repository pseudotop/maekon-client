#!/usr/bin/env bash
set -euo pipefail
# Nightly mutation score check — target: ≥ 70% killed
# Usage: ./scripts/run-mutants.sh [crate-name]
# Default: oneshim-core

CRATE="${1:-oneshim-core}"
echo "=== cargo-mutants: $CRATE ==="
cargo mutants -p "$CRATE" --timeout 120 || true

# Parse results from mutants.out directory (filesystem-based, version-independent)
if [ -d mutants.out ]; then
  caught=$(wc -l < mutants.out/caught.txt 2>/dev/null | tr -d ' ' || echo 0)
  missed=$(wc -l < mutants.out/missed.txt 2>/dev/null | tr -d ' ' || echo 0)
  unviable=$(wc -l < mutants.out/unviable.txt 2>/dev/null | tr -d ' ' || echo 0)
  total=$((caught + missed + unviable))
  if [ "$total" -gt 0 ]; then
    score=$((caught * 100 / total))
    echo "Mutation score: ${score}% (${caught}/${total} caught, ${missed} missed, ${unviable} unviable)"
    if [ "$score" -lt 70 ]; then
      echo "::warning::Mutation score below threshold (70%)"
    fi
  else
    echo "No mutants generated"
  fi
else
  echo "mutants.out directory not found — check cargo-mutants output"
fi
