#!/usr/bin/env bash
set -euo pipefail

echo "=== Maekon Test Health Report ==="
echo ""

# Layer 1: Rust
#
# FAIL-CLOSED (#10003). The previous form was:
#
#   rust_tests=$(cargo test --workspace -- --list 2>/dev/null | grep -c ": test$" || echo "0")
#
# Three defects in one line:
#   1. `2>/dev/null` discarded the cargo error, so a broken workspace looked
#      identical to a healthy one with no tests.
#   2. `set -o pipefail` does not apply inside `$(...)` used this way, so
#      cargo's failure never propagated.
#   3. When cargo failed, `grep -c` printed "0" AND exited 1, so `|| echo "0"`
#      appended a SECOND "0" — producing the literal `Total tests: 0` followed
#      by a stray `0` line described in #10003, while still exiting 0.
#
# A health report that reports zero tests after a build failure — successfully —
# is worse than no report, because the release checklist reads it as evidence.
echo "## Layer 1: Rust Core"
if ! command -v cargo &>/dev/null; then
  echo "::error::cargo not found — test health cannot be reported" >&2
  echo "  FAIL: cargo not found"
  exit 2
fi

cargo_list_log=$(mktemp)
trap 'rm -f "$cargo_list_log"' EXIT

if ! cargo test --workspace -- --list >"$cargo_list_log" 2>&1; then
  echo "::error::cargo test --list failed — refusing to report a test count" >&2
  echo "  FAIL: cargo test --list failed. Output:"
  sed 's/^/    /' "$cargo_list_log" | tail -30
  exit 1
fi

# `grep -c` exits 1 on zero matches; that is a real signal here, not an error to
# swallow, so it is captured separately from the count.
rust_tests=$(grep -c ": test$" "$cargo_list_log" || true)
rust_tests=${rust_tests//[^0-9]/}
: "${rust_tests:=0}"
echo "  Total tests: $rust_tests"

if [ "$rust_tests" -eq 0 ]; then
  echo "::error::cargo test --list succeeded but enumerated 0 tests — treating as an anomaly" >&2
  echo "  FAIL: zero tests enumerated (expected thousands; see #10003)"
  exit 1
fi

# Layer 2: Mock IPC
echo ""
echo "## Layer 2: Frontend Mock IPC"
mock_dir="crates/maekon-web/frontend/src/__tests__"
if [ -d "$mock_dir" ]; then
  mock_files=$(find "$mock_dir" -name "*.test.ts" | wc -l | tr -d ' ')
  echo "  Test files: $mock_files"
else
  echo "  Directory not found"
fi

# Layer 3: Playwright
echo ""
echo "## Layer 3: Playwright"
e2e_dir="crates/maekon-web/frontend/e2e"
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
