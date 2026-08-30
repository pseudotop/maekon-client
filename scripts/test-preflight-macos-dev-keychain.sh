#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PREFLIGHT="$SCRIPT_DIR/preflight-macos-dev-keychain.sh"
FIXTURE_ROOT="$(mktemp -d)"
trap 'rm -rf "$FIXTURE_ROOT"' EXIT

run_preflight() {
  MAEKON_DEV_KEYCHAIN_PROFILE_ROOT="$FIXTURE_ROOT" "$PREFLIGHT" "$@"
}

if run_preflight --flavor dev >"$FIXTURE_ROOT/dev.out" 2>&1; then
  echo "FAIL: shared dev flavor must be rejected" >&2
  exit 1
fi
grep -q '^reason=isolated_flavor_required$' "$FIXTURE_ROOT/dev.out"

if env -u HOME -u MAEKON_DEV_KEYCHAIN_PROFILE_ROOT \
  "$PREFLIGHT" --flavor qc-no-home >"$FIXTURE_ROOT/no-home.out" 2>&1; then
  echo "FAIL: missing profile root must fail closed" >&2
  exit 1
fi
grep -q '^reason=profile_root_unavailable$' "$FIXTURE_ROOT/no-home.out"

mkdir -p "$FIXTURE_ROOT/maekon-dev/data"
printf '{"version":1,"namespaces":{"openai":["access_token"]}}\n' \
  >"$FIXTURE_ROOT/maekon-dev/maekon-keychain-registry.json"
printf '{"version":1,"namespaces":{"storage":["master_key"]}}\n' \
  >"$FIXTURE_ROOT/maekon-dev/data/maekon-master-key-keychain-registry.json"
before_provider="$(cksum "$FIXTURE_ROOT/maekon-dev/maekon-keychain-registry.json")"
before_master="$(cksum "$FIXTURE_ROOT/maekon-dev/data/maekon-master-key-keychain-registry.json")"

run_preflight --flavor qc-demo-20260827 >"$FIXTURE_ROOT/qc.out"
grep -q '^mode=read-only$' "$FIXTURE_ROOT/qc.out"
grep -q '^keychain_queries=0$' "$FIXTURE_ROOT/qc.out"
grep -q '^keychain_mutations=0$' "$FIXTURE_ROOT/qc.out"
grep -q '^legacy_registry_files=2$' "$FIXTURE_ROOT/qc.out"
grep -q '^keychain_service=maekon-qc-demo-20260827$' "$FIXTURE_ROOT/qc.out"
grep -q '^result=pass$' "$FIXTURE_ROOT/qc.out"
[[ "$before_provider" == "$(cksum "$FIXTURE_ROOT/maekon-dev/maekon-keychain-registry.json")" ]]
[[ "$before_master" == "$(cksum "$FIXTURE_ROOT/maekon-dev/data/maekon-master-key-keychain-registry.json")" ]]

mkdir -p "$FIXTURE_ROOT/maekon-qc-used"
if run_preflight --flavor qc-used >"$FIXTURE_ROOT/used.out" 2>&1; then
  echo "FAIL: an existing QC profile must require a fresh flavor" >&2
  exit 1
fi
grep -q '^reason=target_profile_already_exists$' "$FIXTURE_ROOT/used.out"
[[ -d "$FIXTURE_ROOT/maekon-qc-used" ]]

echo "PASS: macOS dev Keychain preflight is isolated, fail-closed, and read-only"
