#!/usr/bin/env bash
set -euo pipefail
# Mutation score gate — target: >= 70% killed (#10003).
#
# Usage:
#   ./scripts/run-mutants.sh [crate-name]          # run cargo-mutants, then score
#   ./scripts/run-mutants.sh --score-only [crate]  # score an existing outcome dir
#
# Env:
#   MUTANTS_OUT_DIR    outcome directory (default: mutants.out)
#   MUTANTS_MIN_SCORE  viable-score threshold, integer percent (default: 70)
#   MUTANTS_RECEIPT    optional path; writes a JSON score receipt for CI artifacts
#
# FAIL-CLOSED CONTRACT (#10003)
# -----------------------------
# The previous version could not fail. It ran `cargo mutants || true`, emitted
# only `::warning::` below the threshold, and printed "No mutants generated"
# (exit 0) when the outcome directory was empty or absent. A release checklist
# that requires ">= 70%" was therefore reading a gate that returned success
# unconditionally.
#
# Every one of these now exits non-zero:
#   - cargo / cargo-mutants not installed             -> 2
#   - outcome directory missing after a run           -> 3
#   - a required outcome file missing or unreadable   -> 3
#   - an outcome file yielding a non-integer count    -> 3
#   - zero classified mutants (nothing was measured)  -> 3
#   - viable score below MUTANTS_MIN_SCORE            -> 1
#
# `--score-only` exists so the scoring/decision half is exercised by
# `test-run-mutants-failclosed.sh` against fixture directories, without a
# multi-hour cargo-mutants run. The logic under test is the SAME code path CI
# takes — not a re-implementation.
#
# ON cargo-mutants EXIT CODES: deliberately NOT interpreted. Upstream uses
# distinct codes for "found missed mutants" vs "internal error", and pinning
# that mapping here would rot silently on a version bump. The run is allowed to
# finish and the OUTCOME FILES are the source of truth: a crashed or aborted run
# leaves them missing/malformed, which fails via the checks above. The raw exit
# code is still recorded in the receipt for triage.

# #11635: crates are collected, not last-wins. The receipt below records this
# list verbatim, so what was scored and what the receipt claims cannot diverge.
CRATES=()
SCORE_ONLY=0
for arg in "$@"; do
  case "$arg" in
    --score-only) SCORE_ONLY=1 ;;
    -*) echo "unknown option: $arg" >&2; exit 2 ;;
    *) CRATES+=("$arg") ;;
  esac
done
# A bare invocation keeps scoring maekon-core — the #10003 release target.
if [ "${#CRATES[@]}" -eq 0 ]; then CRATES=("maekon-core"); fi
CRATE_LABEL="${CRATES[*]}"

OUT_DIR="${MUTANTS_OUT_DIR:-mutants.out}"
MIN_SCORE="${MUTANTS_MIN_SCORE:-70}"
RECEIPT="${MUTANTS_RECEIPT:-}"

die() { echo "::error::$*" >&2; echo "FAIL: $*" >&2; }

# Read one outcome file into a line count. Fails closed: a missing or unreadable
# file is an ERROR, never a silent 0 — "file absent" and "zero mutants of this
# kind" are different facts, and the old `|| echo 0` collapsed them into the
# same value, which is how an aborted run could still print a score.
read_outcome() {
  local name="$1" path="$OUT_DIR/$1.txt"
  if [ ! -f "$path" ]; then
    die "outcome file missing: $path (a crashed or aborted run cannot be scored)"
    exit 3
  fi
  if [ ! -r "$path" ]; then
    die "outcome file unreadable: $path"
    exit 3
  fi
  local count
  count=$(wc -l < "$path" | tr -d '[:space:]')
  if ! [[ "$count" =~ ^[0-9]+$ ]]; then
    die "outcome file produced a non-integer count: $path -> '$count'"
    exit 3
  fi
  printf '%s' "$count"
}

mutants_exit=0

if [ "$SCORE_ONLY" -eq 0 ]; then
  if ! command -v cargo >/dev/null 2>&1; then
    die "cargo not found — cannot run the mutation gate"
    exit 2
  fi
  if ! cargo mutants --version >/dev/null 2>&1; then
    die "cargo-mutants not installed (cargo install cargo-mutants) — the gate cannot be proven"
    exit 2
  fi

  echo "=== cargo-mutants: $CRATE_LABEL (threshold ${MIN_SCORE}%) ==="
  # `-p` is repeated per crate; cargo-mutants does not accept a single "a b".
  crate_args=()
  for crate in "${CRATES[@]}"; do crate_args+=(-p "$crate"); done
  # Intentionally not `|| true`: the code is captured, reported and recorded,
  # but the pass/fail decision is made from the outcome files below.
  set +e
  cargo mutants "${crate_args[@]}" --timeout 120
  mutants_exit=$?
  set -e
  echo "cargo-mutants exit code: ${mutants_exit}"
fi

if [ ! -d "$OUT_DIR" ]; then
  die "outcome directory not found: $OUT_DIR (cargo-mutants produced no results)"
  exit 3
fi

caught=$(read_outcome caught)
missed=$(read_outcome missed)
unviable=$(read_outcome unviable)
# #10003 expectation 2: timeouts are a FOURTH outcome and must be accounted for,
# not silently dropped. `timeout.txt` is always written by cargo-mutants (empty
# when none occurred), so `read_outcome` still fails closed if it is absent.
timeout=$(read_outcome timeout)

# `unviable` mutants never compiled, so they were never a test-detectable
# change. Counting them as failures would penalise the suite for the mutation
# engine's own limitations, so the denominator is the VIABLE set.
#
# `timeout` DOES count toward viable-but-not-caught: the mutant compiled and the
# suite failed to reach a verdict on it. Treating a timeout as caught would let
# an infrastructure stall inflate the score, which is the opposite of what a
# release gate is for. Conservative by design.
viable=$((caught + missed + timeout))
total=$((caught + missed + unviable + timeout))

if [ "$total" -eq 0 ]; then
  die "zero mutants classified in $OUT_DIR — nothing was measured, so no score can be claimed"
  exit 3
fi

if [ "$viable" -eq 0 ]; then
  die "all ${total} mutants were unviable — no viable mutant was tested, so no score can be claimed"
  exit 3
fi

score=$((caught * 100 / viable))

echo "Mutation score: ${score}% (${caught}/${viable} viable caught, ${missed} missed, ${timeout} timeout, ${unviable} unviable, ${total} total)"

if [ -n "$RECEIPT" ]; then
  mkdir -p "$(dirname "$RECEIPT")"
  # #11635: the receipt must name what was actually scored. A dispatch over
  # maekon-automation+maekon-network used to be stamped "maekon-core" (run
  # 33146994661), and the release checklist (#10003) reads this field as the
  # core score's provenance. One crate stays a plain string so existing
  # readers keep working; several become a JSON list.
  if [ "${#CRATES[@]}" -eq 1 ]; then
    crate_json="\"${CRATES[0]}\""
  else
    crate_json="["
    for crate in "${CRATES[@]}"; do crate_json+="\"$crate\","; done
    crate_json="${crate_json%,}]"
  fi
  printf '{"crate":%s,"score":%d,"threshold":%d,"caught":%d,"missed":%d,"timeout":%d,"unviable":%d,"viable":%d,"total":%d,"cargo_mutants_exit":%d}\n' \
    "$crate_json" "$score" "$MIN_SCORE" "$caught" "$missed" "$timeout" "$unviable" "$viable" "$total" "$mutants_exit" \
    > "$RECEIPT"
  echo "receipt: $RECEIPT"
fi

if [ "$score" -lt "$MIN_SCORE" ]; then
  die "mutation score ${score}% is below the required ${MIN_SCORE}%"
  exit 1
fi

echo "PASS: mutation score ${score}% >= ${MIN_SCORE}%"
