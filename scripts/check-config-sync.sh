#!/usr/bin/env bash
# check-config-sync.sh — Port & version consistency checker
#
# Validates that port numbers, version strings, and CSP config
# are synchronized across Rust, frontend, and Tauri config files.
#
# Exit codes:
#   0 — all checks passed
#   1 — one or more mismatches found
#
# Usage:
#   ./scripts/check-config-sync.sh          # run all checks
#   ./scripts/check-config-sync.sh --fix    # show fix suggestions

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ERRORS=0
SHOW_FIX="${1:-}"

red()   { printf '\033[0;31m%s\033[0m\n' "$1"; }
green() { printf '\033[0;32m%s\033[0m\n' "$1"; }
yellow(){ printf '\033[0;33m%s\033[0m\n' "$1"; }
info()  { printf '  %-50s' "$1"; }

fail() {
  red "FAIL"
  ERRORS=$((ERRORS + 1))
  if [ -n "$SHOW_FIX" ] && [ "$SHOW_FIX" = "--fix" ] && [ -n "${2:-}" ]; then
    yellow "  Fix: $2"
  fi
}

pass() { green "OK"; }

echo "=== Config Sync Check ==="
echo ""

# ─── 1. Version Sync ───────────────────────────────────────────────

echo "── Version Sync ──"

# Source of truth: Cargo.toml workspace version
CARGO_VERSION=$(grep -m1 '^version' "$REPO_ROOT/Cargo.toml" | sed 's/.*"\(.*\)".*/\1/')

# package.json version
PKG_JSON="$REPO_ROOT/crates/oneshim-web/frontend/package.json"
if [ -f "$PKG_JSON" ]; then
  PKG_VERSION=$(python3 -c "import json; print(json.load(open('$PKG_JSON'))['version'])" 2>/dev/null || echo "PARSE_ERROR")
  info "Cargo.toml ($CARGO_VERSION) == package.json ($PKG_VERSION)"
  if [ "$CARGO_VERSION" = "$PKG_VERSION" ]; then
    pass
  else
    fail "" "Update package.json version to \"$CARGO_VERSION\""
  fi
else
  info "package.json"
  yellow "SKIP (file not found)"
fi

# src-tauri/Cargo.toml should reference workspace version
TAURI_CARGO="$REPO_ROOT/src-tauri/Cargo.toml"
if [ -f "$TAURI_CARGO" ]; then
  if grep -q 'version\.workspace\s*=\s*true\|version.workspace = true' "$TAURI_CARGO" 2>/dev/null || \
     grep -q 'version\.workspace' "$TAURI_CARGO" 2>/dev/null; then
    info "src-tauri/Cargo.toml uses workspace version"
    pass
  else
    TAURI_VERSION=$(grep -m1 '^version' "$TAURI_CARGO" | sed 's/.*"\(.*\)".*/\1/')
    info "Cargo.toml ($CARGO_VERSION) == src-tauri ($TAURI_VERSION)"
    if [ "$CARGO_VERSION" = "$TAURI_VERSION" ]; then
      pass
    else
      fail "" "Set version.workspace = true in src-tauri/Cargo.toml"
    fi
  fi
fi

echo ""

# ─── 2. Port Sync ──────────────────────────────────────────────────

echo "── Port Sync ──"

# Rust DEFAULT_WEB_PORT (source of truth)
RUST_PORT_FILE="$REPO_ROOT/crates/oneshim-core/src/config/sections/network.rs"
RUST_PORT=$(grep 'DEFAULT_WEB_PORT.*u16.*=' "$RUST_PORT_FILE" | grep -o '[0-9]\{4,5\}' | head -1)

# Frontend constants.ts
TS_CONST_FILE="$REPO_ROOT/crates/oneshim-web/frontend/src/constants.ts"
if [ -f "$TS_CONST_FILE" ]; then
  TS_PORT=$(grep 'DEFAULT_WEB_PORT' "$TS_CONST_FILE" | grep -o '[0-9]\{4,5\}' | head -1)
  info "Rust DEFAULT_WEB_PORT ($RUST_PORT) == constants.ts ($TS_PORT)"
  if [ "$RUST_PORT" = "$TS_PORT" ]; then
    pass
  else
    fail "" "Update constants.ts DEFAULT_WEB_PORT to $RUST_PORT"
  fi
fi

# CSP connect-src must include the default port
TAURI_CONF="$REPO_ROOT/src-tauri/tauri.conf.json"
if [ -f "$TAURI_CONF" ]; then
  CSP_LINE=$(python3 -c "import json; print(json.load(open('$TAURI_CONF'))['app']['security']['csp'])" 2>/dev/null || echo "")
  if echo "$CSP_LINE" | grep -q "127.0.0.1:${RUST_PORT}"; then
    info "CSP connect-src includes port $RUST_PORT"
    pass
  else
    info "CSP connect-src includes port $RUST_PORT"
    fail "" "Add http://127.0.0.1:$RUST_PORT to CSP connect-src in tauri.conf.json"
  fi

  # Check that CSP doesn't include ports outside standard range
  CSP_PORTS=$(echo "$CSP_LINE" | grep -o '127\.0\.0\.1:[0-9]*' | grep -o '[0-9]*$' | sort -u)
  RUST_PORT_BASE=$((RUST_PORT / 10 * 10))  # e.g., 10090 -> 10090
  RUST_PORT_END=$((RUST_PORT_BASE + 9))     # e.g., 10099
  NON_STANDARD=""
  for p in $CSP_PORTS; do
    if [ "$p" -lt "$RUST_PORT_BASE" ] || [ "$p" -gt "$RUST_PORT_END" ]; then
      NON_STANDARD="$NON_STANDARD $p"
    fi
  done
  if [ -z "$NON_STANDARD" ]; then
    info "CSP ports all in standard range ($RUST_PORT_BASE-$RUST_PORT_END)"
    pass
  else
    info "CSP has non-standard ports:$NON_STANDARD"
    fail "" "Remove non-standard ports from CSP connect-src"
  fi
fi

# Standalone fallback port in api-base.ts
API_BASE_FILE="$REPO_ROOT/crates/oneshim-web/frontend/src/utils/api-base.ts"
if [ -f "$API_BASE_FILE" ]; then
  if grep -q 'DEFAULT_WEB_PORT' "$API_BASE_FILE"; then
    info "api-base.ts uses DEFAULT_WEB_PORT (not hardcoded)"
    pass
  else
    API_PORT=$(grep -o '127\.0\.0\.1:[0-9]*' "$API_BASE_FILE" | grep -o '[0-9]*$' | head -1)
    if [ -n "$API_PORT" ] && [ "$API_PORT" != "$RUST_PORT" ]; then
      info "api-base.ts hardcoded port ($API_PORT) != Rust ($RUST_PORT)"
      fail "" "Use DEFAULT_WEB_PORT import instead of hardcoded port"
    fi
  fi
fi

echo ""

# ─── 3. Tauri Config Consistency ───────────────────────────────────

echo "── Tauri Config ──"

if [ -f "$TAURI_CONF" ]; then
  # Window should have visible: false (setup.rs shows it after init)
  VISIBLE=$(python3 -c "import json; w=json.load(open('$TAURI_CONF'))['app']['windows'][0]; print(w.get('visible', True))" 2>/dev/null)
  info "Main window visible=false (deferred show)"
  if [ "$VISIBLE" = "False" ]; then
    pass
  else
    fail "" "Set visible: false in tauri.conf.json windows[0]"
  fi

  # macOS: titleBarStyle should be Overlay
  TITLE_STYLE=$(python3 -c "import json; w=json.load(open('$TAURI_CONF'))['app']['windows'][0]; print(w.get('titleBarStyle', 'MISSING'))" 2>/dev/null)
  info "titleBarStyle = Overlay (macOS native traffic lights)"
  if [ "$TITLE_STYLE" = "Overlay" ]; then
    pass
  else
    fail "" "Set titleBarStyle: \"Overlay\" in tauri.conf.json"
  fi
fi

echo ""

# ─── 4. Frontend build output exists ───────────────────────────────

echo "── Build Artifacts ──"

DIST_DIR="$REPO_ROOT/crates/oneshim-web/frontend/dist"
if [ -d "$DIST_DIR" ] && [ -f "$DIST_DIR/index.html" ]; then
  JS_COUNT=$(find "$DIST_DIR" -name '*.js' | wc -l | tr -d ' ')
  info "Frontend dist/ exists ($JS_COUNT JS files)"
  pass
else
  info "Frontend dist/ exists"
  fail "" "Run: cd crates/oneshim-web/frontend && pnpm build"
fi

echo ""

# ─── Summary ───────────────────────────────────────────────────────

if [ "$ERRORS" -gt 0 ]; then
  red "=== $ERRORS check(s) FAILED ==="
  echo "Run with --fix for suggestions: ./scripts/check-config-sync.sh --fix"
  exit 1
else
  green "=== All checks passed ==="
  exit 0
fi
