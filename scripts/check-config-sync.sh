#!/usr/bin/env bash
# check-config-sync.sh - Port & version consistency checker
#
# Validates that port numbers, version strings, and CSP config
# are synchronized across Rust, frontend, and Tauri config files.
#
# Exit codes:
#   0 - all checks passed
#   1 - one or more mismatches found
#
# Usage:
#   ./scripts/check-config-sync.sh                      # run source/config checks
#   ./scripts/check-config-sync.sh --fix                # show fix suggestions
#   ./scripts/check-config-sync.sh --require-artifacts  # also require frontend dist/

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ERRORS=0
SHOW_FIX=""
REQUIRE_ARTIFACTS=0

for arg in "$@"; do
  case "$arg" in
    --fix) SHOW_FIX="--fix" ;;
    --require-artifacts) REQUIRE_ARTIFACTS=1 ;;
    *)
      printf '\033[0;31m%s\033[0m\n' "Unknown argument: $arg"
      echo "Usage: ./scripts/check-config-sync.sh [--fix] [--require-artifacts]"
      exit 2
      ;;
  esac
done

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

json_value() {
  local file="$1"
  local path="$2"

  if command -v node >/dev/null 2>&1; then
    node -e '
const fs = require("fs");
const [file, path] = process.argv.slice(1);
const data = JSON.parse(fs.readFileSync(file, "utf8"));
let value = data;
for (const segment of path.split(".")) {
  value = value?.[segment];
}
if (value === undefined || value === null) {
  process.exit(2);
}
if (typeof value === "object") {
  console.log(JSON.stringify(value));
} else {
  console.log(String(value));
}
' "$file" "$path"
    return
  fi

  if command -v jq >/dev/null 2>&1; then
    case "$path" in
      version) jq -r '.version // empty' "$file" ;;
      app.security.csp) jq -r '.app.security.csp // empty' "$file" ;;
      app.windows.0.visible) jq -r '.app.windows[0].visible // empty' "$file" ;;
      app.windows.0.titleBarStyle) jq -r '.app.windows[0].titleBarStyle // empty' "$file" ;;
      *) return 2 ;;
    esac
    return
  fi

  if command -v python3 >/dev/null 2>&1; then
    python3 - "$file" "$path" <<'PY'
import json
import sys

file_path, dotted_path = sys.argv[1], sys.argv[2]
with open(file_path, encoding="utf-8") as handle:
    value = json.load(handle)
for segment in dotted_path.split("."):
    if isinstance(value, list):
        value = value[int(segment)]
    else:
        value = value[segment]
print(value)
PY
    return
  fi

  return 127
}

extract_csp_directive_ports() {
  local csp="$1"
  local directive="$2"

  printf '%s\n' "$csp" \
    | tr ';' '\n' \
    | awk -v directive="$directive" '$1 == directive { for (i = 2; i <= NF; i++) print $i }' \
    | grep -oE '127\.0\.0\.1:[0-9]+' \
    | grep -oE '[0-9]+$' \
    | sort -n \
    | uniq || true
}

missing_ports_in_range() {
  local ports="$1"
  local start="$2"
  local end="$3"
  local missing=""
  local port="$start"

  while [ "$port" -le "$end" ]; do
    if ! printf '%s\n' "$ports" | grep -qx "$port"; then
      missing="$missing $port"
    fi
    port=$((port + 1))
  done

  printf '%s\n' "$missing"
}

echo "=== Config Sync Check ==="
echo ""

# 1. Version Sync

echo "-- Version Sync --"

# Source of truth: Cargo.toml workspace version
CARGO_VERSION=$(grep -m1 '^version' "$REPO_ROOT/Cargo.toml" | sed 's/.*"\(.*\)".*/\1/')

# package.json version
PKG_JSON="$REPO_ROOT/crates/maekon-web/frontend/package.json"
if [ -f "$PKG_JSON" ]; then
  PKG_VERSION=$(json_value "$PKG_JSON" version 2>/dev/null || echo "PARSE_ERROR")
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

# 2. Port Sync

echo "-- Port Sync --"

# Rust DEFAULT_WEB_PORT (source of truth)
RUST_PORT_FILE="$REPO_ROOT/crates/maekon-core/src/config/sections/network.rs"
RUST_PORT=$(grep 'DEFAULT_WEB_PORT.*u16.*=' "$RUST_PORT_FILE" | grep -o '[0-9]\{4,5\}' | head -1)

# Frontend constants.ts
TS_CONST_FILE="$REPO_ROOT/crates/maekon-web/frontend/src/constants.ts"
if [ -f "$TS_CONST_FILE" ]; then
  TS_PORT=$(grep 'DEFAULT_WEB_PORT' "$TS_CONST_FILE" | grep -o '[0-9]\{4,5\}' | head -1)
  info "Rust DEFAULT_WEB_PORT ($RUST_PORT) == constants.ts ($TS_PORT)"
  if [ "$RUST_PORT" = "$TS_PORT" ]; then
    pass
  else
    fail "" "Update constants.ts DEFAULT_WEB_PORT to $RUST_PORT"
  fi
fi

# Production and dev CSP must both include the loopback dashboard range. Tauri v2
# replaces the dev CSP leaf instead of deep-merging it, so each config must carry
# the local Axum API/SSE and frame image origins explicitly.
TAURI_CONF="$REPO_ROOT/src-tauri/tauri.conf.json"
TAURI_DEV_CONF="$REPO_ROOT/src-tauri/tauri.dev.conf.json"
if [ -f "$TAURI_CONF" ]; then
  PROD_CSP=$(json_value "$TAURI_CONF" app.security.csp 2>/dev/null || echo "")

  RUST_PORT_BASE=$((RUST_PORT / 10 * 10))  # e.g., 10090 -> 10090
  RUST_PORT_END=$((RUST_PORT_BASE + 9))     # e.g., 10099

  PROD_CONNECT_PORTS=$(extract_csp_directive_ports "$PROD_CSP" connect-src)
  PROD_IMG_PORTS=$(extract_csp_directive_ports "$PROD_CSP" img-src)
  PROD_CONNECT_MISSING=$(missing_ports_in_range "$PROD_CONNECT_PORTS" "$RUST_PORT_BASE" "$RUST_PORT_END")
  PROD_IMG_MISSING=$(missing_ports_in_range "$PROD_IMG_PORTS" "$RUST_PORT_BASE" "$RUST_PORT_END")
  if [ -z "$PROD_CONNECT_MISSING" ] && [ -z "$PROD_IMG_MISSING" ]; then
    info "Prod CSP allows dashboard ports ($RUST_PORT_BASE-$RUST_PORT_END)"
    pass
  else
    info "Prod CSP allows dashboard ports ($RUST_PORT_BASE-$RUST_PORT_END)"
    fail "" "Add missing production CSP ports: connect-src:$PROD_CONNECT_MISSING img-src:$PROD_IMG_MISSING"
  fi

  DEV_CSP=""
  if [ -f "$TAURI_DEV_CONF" ]; then
    DEV_CSP=$(json_value "$TAURI_DEV_CONF" app.security.csp 2>/dev/null || echo "")
    DEV_CONNECT_PORTS=$(extract_csp_directive_ports "$DEV_CSP" connect-src)
    DEV_IMG_PORTS=$(extract_csp_directive_ports "$DEV_CSP" img-src)
    DEV_CONNECT_MISSING=$(missing_ports_in_range "$DEV_CONNECT_PORTS" "$RUST_PORT_BASE" "$RUST_PORT_END")
    DEV_IMG_MISSING=$(missing_ports_in_range "$DEV_IMG_PORTS" "$RUST_PORT_BASE" "$RUST_PORT_END")
    if [ -z "$DEV_CONNECT_MISSING" ] && [ -z "$DEV_IMG_MISSING" ]; then
      info "Dev CSP allows dashboard ports"
      pass
    else
      info "Dev CSP allows dashboard ports"
      fail "" "Add missing dev CSP ports: connect-src:$DEV_CONNECT_MISSING img-src:$DEV_IMG_MISSING"
    fi
  fi

  # Check that configured localhost CSP ports stay inside the dashboard fallback range.
  CSP_PORTS=$(printf '%s\n%s\n' "$PROD_CSP" "$DEV_CSP" | grep -o '127\.0\.0\.1:[0-9]*' | grep -o '[0-9]*$' | sort -u || true)
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
API_BASE_FILE="$REPO_ROOT/crates/maekon-web/frontend/src/utils/api-base.ts"
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

# 3. Tauri Config Consistency

echo "-- Tauri Config --"

if [ -f "$TAURI_CONF" ]; then
  # Window should have visible: false (setup.rs shows it after init)
  VISIBLE=$(json_value "$TAURI_CONF" app.windows.0.visible 2>/dev/null || echo "true")
  info "Main window visible=false (deferred show)"
  if [ "$VISIBLE" = "False" ] || [ "$VISIBLE" = "false" ]; then
    pass
  else
    fail "" "Set visible: false in tauri.conf.json windows[0]"
  fi

  # macOS: titleBarStyle should be Overlay
  TITLE_STYLE=$(json_value "$TAURI_CONF" app.windows.0.titleBarStyle 2>/dev/null || echo "MISSING")
  info "titleBarStyle = Overlay (macOS native traffic lights)"
  if [ "$TITLE_STYLE" = "Overlay" ]; then
    pass
  else
    fail "" "Set titleBarStyle: \"Overlay\" in tauri.conf.json"
  fi
fi

echo ""

# 4. Frontend build output exists

echo "-- Build Artifacts --"

DIST_DIR="$REPO_ROOT/crates/maekon-web/frontend/dist"
if [ -d "$DIST_DIR" ] && [ -f "$DIST_DIR/index.html" ]; then
  JS_COUNT=$(find "$DIST_DIR" -name '*.js' | wc -l | tr -d ' ')
  if [ "$REQUIRE_ARTIFACTS" -eq 1 ] && [ "$JS_COUNT" -eq 0 ]; then
    info "Frontend dist/ has no JavaScript artifacts"
    fail "" "Run: cd crates/maekon-web/frontend && pnpm build"
  else
    info "Frontend dist/ exists ($JS_COUNT JS files)"
    pass
  fi
else
  info "Frontend dist/ exists"
  if [ "$REQUIRE_ARTIFACTS" -eq 1 ]; then
    fail "" "Run: cd crates/maekon-web/frontend && pnpm build"
  else
    yellow "SKIP (not required; run with --require-artifacts after pnpm build)"
  fi
fi

echo ""

# Summary

if [ "$ERRORS" -gt 0 ]; then
  red "=== $ERRORS check(s) FAILED ==="
  echo "Run with --fix for suggestions: ./scripts/check-config-sync.sh --fix"
  exit 1
else
  green "=== All checks passed ==="
  exit 0
fi
