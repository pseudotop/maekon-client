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
PKG_JSON="crates/oneshim-web/frontend/package.json"
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
  if changelog_has_entry "$VERSION"; then
    pass "CHANGELOG.md has entry for [$VERSION]"
    # Check if it has a date
    if grep -q "## \[$VERSION\] - " CHANGELOG.md; then
      pass "CHANGELOG entry has a date"
    else
      warn "CHANGELOG entry for [$VERSION] has no date"
    fi
    # Check if section has content (not just the header)
    SECTION_LINE=$(grep -n "## \[$VERSION\]" CHANGELOG.md | head -1 | cut -d: -f1)
    NEXT_SECTION_LINE=$(awk -v start="$((SECTION_LINE + 1))" 'NR > start && /^## \[/ { print NR; exit }' CHANGELOG.md)
    if [ -n "$NEXT_SECTION_LINE" ]; then
      CONTENT_LINES=$(sed -n "$((SECTION_LINE + 1)),$((NEXT_SECTION_LINE - 1))p" CHANGELOG.md | grep -c '[^[:space:]]' || true)
    else
      CONTENT_LINES=$(sed -n "$((SECTION_LINE + 1)),\$p" CHANGELOG.md | grep -c '[^[:space:]]' || true)
    fi
    if [ "$CONTENT_LINES" -gt 2 ]; then
      pass "CHANGELOG section has content ($CONTENT_LINES non-empty lines)"
    else
      warn "CHANGELOG section looks sparse ($CONTENT_LINES non-empty lines)"
    fi
  else
    fail "CHANGELOG.md missing entry for [$VERSION]"
    echo "       Fix: Add '## [$VERSION] - $(date +%Y-%m-%d)' section to CHANGELOG.md"
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

    mapfile -t CHANGED_FILES < <(git diff --name-only "$RC_COMMIT" "$HEAD_COMMIT")
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

# --- 7. Run config-sync check if available ---
if [ -x "scripts/check-config-sync.sh" ]; then
  echo "[Config Sync]"
  if scripts/check-config-sync.sh > /dev/null 2>&1; then
    pass "Config sync check passed"
  else
    fail "Config sync check failed — run: ./scripts/check-config-sync.sh --fix"
  fi
  echo ""
fi

# --- Frontend Build Verification ---
FRONTEND_DIR="crates/oneshim-web/frontend"
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

# --- Dependency Security Gate ---
echo "[Dependency Security]"
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
  ALERT_EXIT=0
  ALERT_RESPONSE=$(gh api repos/{owner}/{repo}/dependabot/alerts 2>&1) || ALERT_EXIT=$?
  if [ "$ALERT_EXIT" -ne 0 ]; then
    if echo "$ALERT_RESPONSE" | grep -qiE "dependabot.*(disabled|not enabled)|feature.*disabled|is not enabled"; then
      warn "Dependabot alerts not enabled for this repo — security gate cannot run (enable in repo Settings > Code security)"
    else
      warn "Dependabot alerts query failed (exit $ALERT_EXIT): $(echo "$ALERT_RESPONSE" | head -c 200)"
    fi
  else
    ALERT_COUNT=$(echo "$ALERT_RESPONSE" | jq '[.[] | select(.state == "open")] | length' 2>/dev/null || echo "parse-error")
    if [ "$ALERT_COUNT" = "parse-error" ]; then
      warn "Dependabot alerts response parse failed — unexpected schema"
    elif [ "$ALERT_COUNT" -gt 0 ]; then
      fail "Open dependabot security alerts: $ALERT_COUNT — resolve before releasing"
    else
      pass "No open dependabot security alerts"
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
