#!/usr/bin/env bash
# pre-release-check.sh — Run before tagging a release
# Validates version consistency + CHANGELOG entry
# Usage: ./scripts/pre-release-check.sh [version]
#   version: e.g. "0.3.5" (without v prefix). If omitted, reads from Cargo.toml.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"
source "${REPO_ROOT}/scripts/release-common.sh"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
NC='\033[0m'

errors=0
warnings=0

pass() { echo -e "  ${GREEN}✓${NC} $1"; }
fail() { echo -e "  ${RED}✗${NC} $1"; errors=$((errors + 1)); }
warn() { echo -e "  ${YELLOW}!${NC} $1"; warnings=$((warnings + 1)); }

PYTHON_CMD=()
resolve_python_cmd() {
  if [ -n "${PYTHON_BIN:-}" ]; then
    PYTHON_CMD=("$PYTHON_BIN")
    return 0
  fi

  local candidate
  for candidate in python3 python; do
    if command -v "$candidate" >/dev/null 2>&1; then
      PYTHON_CMD=("$candidate")
      return 0
    fi
  done

  if command -v py >/dev/null 2>&1; then
    PYTHON_CMD=(py -3)
    return 0
  fi

  return 1
}

run_python() {
  if [ "${#PYTHON_CMD[@]}" -eq 0 ]; then
    echo "Python 3 is required for release validation" >&2
    return 127
  fi
  "${PYTHON_CMD[@]}" "$@"
}

resolve_python_cmd || true

# --- Resolve version ---
CARGO_VERSION="$(workspace_version)"

if [ -n "${1:-}" ]; then
  VERSION="$1"
else
  VERSION="$CARGO_VERSION"
fi

if is_rc_version "$VERSION"; then
  RELEASE_KIND="rc"
  BASE_VERSION="$(base_version "$VERSION")"
elif is_stable_version "$VERSION"; then
  RELEASE_KIND="stable"
  BASE_VERSION="$VERSION"
else
  echo "Unsupported version format: $VERSION"
  echo "Expected x.y.z-rc.N or x.y.z"
  exit 1
fi

echo "=== Pre-Release Check: v${VERSION} ==="
echo ""

# --- 1. Cargo.toml version ---
echo "[Version Sync]"
if [ "$CARGO_VERSION" = "$VERSION" ]; then
  pass "Cargo.toml workspace version: $CARGO_VERSION"
else
  fail "Cargo.toml version ($CARGO_VERSION) != target ($VERSION)"
fi

CURRENT_BRANCH=$(git rev-parse --abbrev-ref HEAD)
if [ "$CURRENT_BRANCH" = "main" ]; then
  pass "Current branch: main"
else
  fail "Release tags must be created from main (current: $CURRENT_BRANCH)"
fi

# --- 2. package.json version ---
PKG_JSON="crates/maekon-web/frontend/package.json"
if [ -f "$PKG_JSON" ]; then
  PKG_VERSION="$(frontend_version)"
  if [ "$PKG_VERSION" = "$VERSION" ]; then
    pass "package.json version: $PKG_VERSION"
  else
    fail "package.json version ($PKG_VERSION) != target ($VERSION)"
    echo "       Fix: Update version in $PKG_JSON to \"$VERSION\""
  fi
fi

# --- 3. src-tauri Cargo.toml inherits workspace version ---
TAURI_CARGO="src-tauri/Cargo.toml"
if [ -f "$TAURI_CARGO" ]; then
  if grep -q 'version.workspace = true' "$TAURI_CARGO"; then
    pass "src-tauri/Cargo.toml inherits workspace version"
  else
    TAURI_VER=$(grep -m1 '^version' "$TAURI_CARGO" | sed 's/.*"\(.*\)"/\1/')
    if [ "$TAURI_VER" = "$VERSION" ]; then
      pass "src-tauri/Cargo.toml version: $TAURI_VER"
    else
      fail "src-tauri/Cargo.toml version ($TAURI_VER) != target ($VERSION)"
    fi
  fi
fi

echo ""

# --- 4. CHANGELOG.md ---
echo "[CHANGELOG]"
if [ -f "CHANGELOG.md" ]; then
  if POLICY_OUTPUT="$(scripts/verify-release-notes-policy.sh --public --version "$VERSION" 2>&1)"; then
    pass "CHANGELOG release notes policy passed"
    printf '%s\n' "$POLICY_OUTPUT" | sed 's/^/       /'
  else
    fail "CHANGELOG release notes policy failed for [$VERSION]"
    printf '%s\n' "$POLICY_OUTPUT" | sed 's/^/       /'
    echo "       Fix: Add '## [$VERSION] - $(date +%Y-%m-%d)' with reviewed release notes to CHANGELOG.md"
  fi
else
  fail "CHANGELOG.md not found"
fi

echo ""

# --- 4b. Release policy ---
echo "[Release Policy]"
if [ "$RELEASE_KIND" = "rc" ]; then
  pass "Release type: release candidate"
  if git tag -l "v$BASE_VERSION" | grep -q "^v$BASE_VERSION$"; then
    fail "Stable tag v$BASE_VERSION already exists; new RCs for the same base version are not allowed"
  else
    pass "Stable tag v$BASE_VERSION does not exist yet"
  fi
else
  pass "Release type: stable promotion"
  RC_TAG="$(latest_rc_tag_for_base "$BASE_VERSION")"
  if [ -z "$RC_TAG" ]; then
    fail "No RC tag found for $BASE_VERSION (expected v$BASE_VERSION-rc.N first)"
  else
    pass "Latest RC tag: $RC_TAG"
    RC_COMMIT="$(git rev-parse "${RC_TAG}^{commit}")"
    HEAD_COMMIT="$(git rev-parse HEAD)"
    if [ "$RC_COMMIT" = "$HEAD_COMMIT" ]; then
      fail "Stable tag must be created from a promotion commit, not directly from $RC_TAG"
    fi

    CHANGED_FILES=()
    while IFS= read -r file; do
      CHANGED_FILES+=("$file")
    done < <(git diff --name-only "$RC_COMMIT" "$HEAD_COMMIT")
    if [ "${#CHANGED_FILES[@]}" -eq 0 ]; then
      fail "Stable promotion commit must change metadata files relative to $RC_TAG"
    else
      pass "Files changed since $RC_TAG: ${#CHANGED_FILES[@]}"
      BAD_FILES=()
      for file in "${CHANGED_FILES[@]}"; do
        [ -z "$file" ] && continue
        if ! allowed_promotion_file "$file"; then
          BAD_FILES+=("$file")
        fi
      done
      if [ "${#BAD_FILES[@]}" -gt 0 ]; then
        fail "Stable promotion changed non-metadata files: ${BAD_FILES[*]}"
      else
        pass "Stable promotion changed metadata files only"
      fi
    fi

    if changelog_section_body_matches "$BASE_VERSION" "${RC_TAG#v}"; then
      pass "Stable CHANGELOG entry matches the latest RC section"
    else
      fail "Stable CHANGELOG entry must match the latest RC section [${RC_TAG#v}]"
    fi
  fi
fi

echo ""

# --- 5. Git status ---
echo "[Git Status]"
if [ -z "$(git status --porcelain)" ]; then
  pass "Working tree is clean"
else
  fail "Working tree has uncommitted changes — commit or stash before tagging"
fi

if [ -z "$(git diff --cached --name-only)" ]; then
  pass "No staged changes"
else
  warn "Staged changes exist — commit before tagging"
fi

echo ""

# --- 6. Tag check ---
echo "[Tag]"
if git tag -l "v$VERSION" | grep -q "v$VERSION"; then
  warn "Tag v$VERSION already exists (re-tagging will require: git tag -d v$VERSION && git push origin :refs/tags/v$VERSION)"
else
  pass "Tag v$VERSION does not exist yet"
fi

echo ""

# --- Frontend Build Verification ---
FRONTEND_DIR="crates/maekon-web/frontend"
if [ -d "$FRONTEND_DIR/node_modules" ]; then
  echo "[Frontend Build]"
  if (cd "$FRONTEND_DIR" && npx tsc --noEmit > /dev/null 2>&1); then
    pass "TypeScript type check passed"
  else
    fail "TypeScript type check failed — run: cd $FRONTEND_DIR && npx tsc --noEmit"
  fi
  if (cd "$FRONTEND_DIR" && npx vite build > /dev/null 2>&1); then
    pass "Vite build passed"
  else
    fail "Vite build failed — run: cd $FRONTEND_DIR && pnpm build"
  fi
  echo ""
else
  echo "[Frontend Build]"
  warn "Skipped — node_modules not installed (run: cd $FRONTEND_DIR && pnpm install)"
  echo ""
fi

# --- 7. Run config-sync check if available ---
if [ -x "scripts/check-config-sync.sh" ]; then
  echo "[Config Sync]"
  if [ "${MAEKON_RELEASE_REQUIRE_ARTIFACTS:-0}" = "1" ]; then
    if scripts/check-config-sync.sh --require-artifacts > /dev/null 2>&1; then
      pass "Config sync check passed (artifacts required)"
    else
      fail "Config sync artifact check failed — run: ./scripts/check-config-sync.sh --fix --require-artifacts"
    fi
  else
    if scripts/check-config-sync.sh > /dev/null 2>&1; then
      pass "Config sync source/config check passed"
    else
      fail "Config sync source/config check failed — run: ./scripts/check-config-sync.sh --fix"
    fi
  fi
  echo ""
fi

# --- Desktop Release Decision Gate ---
echo "[Desktop Release Decision]"
if [ -z "${MAEKON_RELEASE_DECISION_MANIFEST:-}" ]; then
  fail "MAEKON_RELEASE_DECISION_MANIFEST must point to an accepted E19 release-decision manifest"
elif [ ! -f "$MAEKON_RELEASE_DECISION_MANIFEST" ]; then
  fail "Release-decision manifest not found: $MAEKON_RELEASE_DECISION_MANIFEST"
elif ! run_python scripts/release_decision_manifest.py validate --manifest "$MAEKON_RELEASE_DECISION_MANIFEST"; then
  fail "Release-decision manifest failed contract validation"
elif run_python - "$MAEKON_RELEASE_DECISION_MANIFEST" "v$VERSION" "$(git rev-parse HEAD)" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as fh:
    manifest = json.load(fh)

expected_tag = sys.argv[2]
expected_sha = sys.argv[3]
state = (manifest.get("release_decision") or {}).get("state")
if state != "pass":
    raise SystemExit(f"release_decision.state must be pass, got {state}")
if manifest.get("release_tag") != expected_tag:
    raise SystemExit(f"release_tag must be {expected_tag}, got {manifest.get('release_tag')}")
if manifest.get("commit_sha") != expected_sha:
    raise SystemExit(f"commit_sha must be {expected_sha}, got {manifest.get('commit_sha')}")
PY
then
  pass "Release-decision manifest accepted"
else
  fail "Release-decision manifest is not release-accepted"
fi
echo ""

# --- Dependency Security Gate ---
echo "[Dependency Security]"
if scripts/check-webdriver-security-isolation.sh; then
  pass "WebDriver-only vulnerable dependency isolation verified"
else
  fail "WebDriver dependency isolation failed — shipped artifacts may contain test-only GTK3 dependencies"
fi

if command -v gh >/dev/null 2>&1; then
  # Check for open security advisories via dependabot alerts.
  # Distinguish genuine "0 open alerts" from "Dependabot disabled / query failed" —
  # the original `|| echo "0"` fallback silently passed in the disabled case.
  #
  # `gh api` exits non-zero when dependabot is disabled (HTTP 403) or when the
  # endpoint is otherwise unreachable. With `set -euo pipefail` (line 7) at
  # script scope, a bare `var=$(failing_cmd)` aborts the script before
  # `ALERT_EXIT=$?` can run, producing a silent exit. Anchor the assignment
  # in an `|| ALERT_EXIT=$?` clause: bash's errexit is suppressed for the
  # left side of an `||` list, and the OR captures the substitution's exit
  # status into `ALERT_EXIT` so the branches below can classify the failure.
  ACCEPTANCE_FILE="supply-chain/release-alert-acceptance.json"
  if [ ! -f "$ACCEPTANCE_FILE" ]; then
    fail "$ACCEPTANCE_FILE missing — accepted release alerts must be explicit"
  else
    DEPENDABOT_ALERTS_FILE="$(mktemp)"
    CODEQL_ALERTS_FILE="$(mktemp)"
    DEPENDABOT_ERROR_FILE="$(mktemp)"
    CODEQL_ERROR_FILE="$(mktemp)"
    cleanup_alert_files() {
      rm -f "$DEPENDABOT_ALERTS_FILE" "$CODEQL_ALERTS_FILE" "$DEPENDABOT_ERROR_FILE" "$CODEQL_ERROR_FILE" 2>/dev/null || true
    }
    trap cleanup_alert_files EXIT

    ALERT_EXIT=0
    gh api --paginate --slurp "repos/{owner}/{repo}/dependabot/alerts?state=open&per_page=100" > "$DEPENDABOT_ALERTS_FILE" 2>"$DEPENDABOT_ERROR_FILE" || ALERT_EXIT=$?
    if [ "$ALERT_EXIT" -ne 0 ]; then
      ALERT_RESPONSE="$(cat "$DEPENDABOT_ERROR_FILE")"
      if echo "$ALERT_RESPONSE" | grep -qiE "dependabot.*(disabled|not enabled)|feature.*disabled|is not enabled"; then
        warn "Dependabot alerts not enabled for this repo — release workflow will fail until the gate can run"
      else
        warn "Dependabot alerts query failed (exit $ALERT_EXIT): $(echo "$ALERT_RESPONSE" | head -c 200)"
      fi
    fi

    CODEQL_EXIT=0
    gh api --paginate --slurp "repos/{owner}/{repo}/code-scanning/alerts?state=open&per_page=100" > "$CODEQL_ALERTS_FILE" 2>"$CODEQL_ERROR_FILE" || CODEQL_EXIT=$?
    ALERT_QUERY_FAILED=0
    if [ "$ALERT_EXIT" -ne 0 ]; then
      ALERT_QUERY_FAILED=1
    fi

    if [ "$CODEQL_EXIT" -ne 0 ]; then
      ALERT_QUERY_FAILED=1
      CODEQL_RESPONSE="$(cat "$CODEQL_ERROR_FILE")"
      warn "CodeQL alerts query failed (exit $CODEQL_EXIT): $(echo "$CODEQL_RESPONSE" | head -c 200)"
    fi

    if [ "$ALERT_QUERY_FAILED" -ne 0 ]; then
      fail "Release security alert query failed — cannot prove Dependabot/CodeQL queues are clean"
    else
      if run_python - "$ACCEPTANCE_FILE" "$DEPENDABOT_ALERTS_FILE" "$CODEQL_ALERTS_FILE" <<'PY'
import datetime as dt
import json
import sys

acceptance_path, dependabot_path, codeql_path = sys.argv[1:]
today = dt.date.today()

with open(acceptance_path, encoding="utf-8") as fh:
    accepted = json.load(fh)

def load_paginated_alerts(path):
    with open(path, encoding="utf-8") as fh:
        data = json.load(fh)
    if isinstance(data, list) and all(isinstance(page, list) for page in data):
        return [alert for page in data for alert in page]
    if isinstance(data, list):
        return data
    raise TypeError(f"{path} must contain a JSON array or paginated array")

dependabot_alerts = load_paginated_alerts(dependabot_path)
codeql_alerts = load_paginated_alerts(codeql_path)

def not_expired(entry):
    value = entry.get("accepted_until")
    return bool(value) and dt.date.fromisoformat(value) >= today

def accepted_dependabot(alert):
    advisory = alert.get("security_advisory") or {}
    dependency = alert.get("dependency") or {}
    package = (dependency.get("package") or {}).get("name")
    manifest = dependency.get("manifest_path")
    for entry in accepted.get("dependabot", []):
        if not not_expired(entry):
            continue
        selectors = {
            "number": alert.get("number"),
            "advisory": advisory.get("ghsa_id"),
            "package": package,
            "manifest": manifest,
        }
        declared = [(key, value) for key, value in selectors.items() if key in entry]
        if declared and all(entry.get(key) == value for key, value in declared):
            return True
    return False

def accepted_codeql(alert):
    rule = alert.get("rule") or {}
    for entry in accepted.get("codeql", []):
        if not not_expired(entry):
            continue
        if entry.get("number") == alert.get("number"):
            return True
        if entry.get("rule") == rule.get("id"):
            return True
    return False

open_dependabot = [a for a in dependabot_alerts if not accepted_dependabot(a)]
open_codeql = [
    a
    for a in codeql_alerts
    if (a.get("tool") or {}).get("name") == "CodeQL" and not accepted_codeql(a)
]

for alert in open_dependabot:
    advisory = alert.get("security_advisory") or {}
    dependency = alert.get("dependency") or {}
    package = (dependency.get("package") or {}).get("name", "unknown")
    print(
        f"unaccepted Dependabot alert #{alert.get('number')}: "
        f"{package} {advisory.get('severity', 'unknown')} "
        f"{advisory.get('ghsa_id', 'unknown')}"
    )

for alert in open_codeql:
    rule = alert.get("rule") or {}
    print(f"unaccepted CodeQL alert #{alert.get('number')}: {rule.get('id', 'unknown rule')}")

raise SystemExit(1 if open_dependabot or open_codeql else 0)
PY
      then
        pass "No unaccepted Dependabot or CodeQL alerts"
      else
        fail "Unaccepted release security alerts are open — resolve or record acceptance"
      fi
    fi
  fi

  # Check for open dependency PRs
  OPEN_DEP_PRS=$(gh pr list --label dependencies --state open --json number --jq 'length' 2>/dev/null || echo "0")
  if [ "$OPEN_DEP_PRS" -gt 0 ] && [ "$OPEN_DEP_PRS" != "0" ]; then
    warn "Open dependency PRs: $OPEN_DEP_PRS — consider resolving before release"
  else
    pass "No open dependency PRs"
  fi
else
  warn "gh CLI not available — skipping dependency security check"
fi
echo ""

# --- Summary ---
echo "=== Summary ==="
if [ "$errors" -gt 0 ]; then
  echo -e "${RED}$errors error(s)${NC}, $warnings warning(s) — fix errors before tagging"
  exit 1
elif [ "$warnings" -gt 0 ]; then
  echo -e "${GREEN}0 errors${NC}, ${YELLOW}$warnings warning(s)${NC} — OK to proceed"
  exit 0
else
  echo -e "${GREEN}All checks passed${NC} — ready to tag v$VERSION"
  exit 0
fi
