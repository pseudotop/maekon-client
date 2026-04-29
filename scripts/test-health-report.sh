#!/usr/bin/env bash
set -euo pipefail

echo "=== Maekon Test Health Report ==="
echo ""

# Layer 1: Rust
echo "## Layer 1: Rust Core"
if command -v cargo &>/dev/null; then
  rust_tests=$(cargo test --workspace -- --list 2>/dev/null | grep -c ": test$" || echo "0")
  echo "  Total tests: $rust_tests"
else
  echo "  cargo not found"
fi

# Layer 2: Mock IPC
echo ""
echo "## Layer 2: Frontend Mock IPC"
mock_dir="crates/oneshim-web/frontend/src/__tests__"
if [ -d "$mock_dir" ]; then
  mock_files=$(find "$mock_dir" -name "*.test.ts" | wc -l | tr -d ' ')
  echo "  Test files: $mock_files"
else
  echo "  Directory not found"
fi

# Layer 3: Playwright
echo ""
echo "## Layer 3: Playwright"
e2e_dir="crates/oneshim-web/frontend/e2e"
if [ -d "$e2e_dir" ]; then
  pw_files=$(find "$e2e_dir" -name "*.spec.ts" | wc -l | tr -d ' ')
  echo "  Spec files: $pw_files"
else
  echo "  Directory not found"
fi

# Layer 4: Tauri WDIO
echo ""
echo "## Layer 4: Tauri Desktop E2E"
echo "  (run separately — run-e2e-tauri.sh)"

echo ""
echo "## Quarantined Tests (@flaky)"
grep -r "@flaky\|#\[ignore\].*flaky" crates/ tests/ --include="*.rs" --include="*.ts" -l 2>/dev/null || echo "  None"

echo ""
echo "## Flaky Quarantine Policy"
echo "  1. Flaky = fails ≥ 3x/week without code changes"
echo "  2. Tag: @flaky (TS) or #[ignore] // flaky: #issue (Rust)"
echo "  3. Each quarantined test MUST have a linked issue"
echo "  4. Excluded from pre-merge, included in nightly"
echo "  5. Max quarantine: 30 days — fix or delete with justification"
