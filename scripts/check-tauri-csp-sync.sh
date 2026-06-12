#!/usr/bin/env bash
# check-tauri-csp-sync.sh - validate production/dev Tauri CSP port policy.
#
# Tauri v2 replaces the CSP leaf string instead of deep-merging it. Production
# and dev CSP must each carry the local dashboard API/SSE and frame image
# loopback range explicitly.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

PROD_CONF="$REPO_ROOT/src-tauri/tauri.conf.json"
DEV_CONF="$REPO_ROOT/src-tauri/tauri.dev.conf.json"
RUST_PORT_FILE="$REPO_ROOT/crates/maekon-core/src/config/sections/network.rs"
RUST_PORT=$(grep 'DEFAULT_WEB_PORT.*u16.*=' "$RUST_PORT_FILE" | grep -o '[0-9]\{4,5\}' | head -1)
RUST_PORT_BASE=$((RUST_PORT / 10 * 10))
RUST_PORT_END=$((RUST_PORT_BASE + 9))

red()   { printf '\033[0;31m%s\033[0m\n' "$1"; }
green() { printf '\033[0;32m%s\033[0m\n' "$1"; }
yellow(){ printf '\033[0;33m%s\033[0m\n' "$1"; }

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
console.log(String(value));
' "$file" "$path"
    return
  fi

  if command -v jq >/dev/null 2>&1; then
    case "$path" in
      app.security.csp) jq -r '.app.security.csp // empty' "$file" ;;
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
    value = value[segment]
print(value)
PY
    return
  fi

  return 127
}

extract_ports() {
  local csp="$1"
  printf '%s\n' "$csp" | grep -oE '127\.0\.0\.1:[0-9]+' | grep -oE '[0-9]+$' | sort -n | uniq || true
}

extract_directive_ports() {
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

ports_in_range() {
  local ports="$1"
  local outside=""
  local port

  for port in $ports; do
    if [ "$port" -lt "$RUST_PORT_BASE" ] || [ "$port" -gt "$RUST_PORT_END" ]; then
      outside="$outside $port"
    fi
  done

  printf '%s\n' "$outside"
}

missing_ports_in_range() {
  local ports="$1"
  local missing=""
  local port="$RUST_PORT_BASE"

  while [ "$port" -le "$RUST_PORT_END" ]; do
    if ! printf '%s\n' "$ports" | grep -qx "$port"; then
      missing="$missing $port"
    fi
    port=$((port + 1))
  done

  printf '%s\n' "$missing"
}

echo "=== Tauri CSP Port Policy Check ==="
echo ""

PROD_CSP="$(json_value "$PROD_CONF" app.security.csp 2>/dev/null || true)"
DEV_CSP="$(json_value "$DEV_CONF" app.security.csp 2>/dev/null || true)"

if [ -z "$PROD_CSP" ]; then
  red "ERROR: app.security.csp not found in tauri.conf.json"
  exit 1
fi

if [ -z "$DEV_CSP" ]; then
  red "ERROR: app.security.csp not found in tauri.dev.conf.json"
  exit 1
fi

PROD_PORTS="$(extract_ports "$PROD_CSP")"
DEV_PORTS="$(extract_ports "$DEV_CSP")"
ALL_PORTS="$(printf '%s\n%s\n' "$PROD_PORTS" "$DEV_PORTS" | sort -n | uniq)"
PROD_CONNECT_PORTS="$(extract_directive_ports "$PROD_CSP" connect-src)"
PROD_IMG_PORTS="$(extract_directive_ports "$PROD_CSP" img-src)"
DEV_CONNECT_PORTS="$(extract_directive_ports "$DEV_CSP" connect-src)"
DEV_IMG_PORTS="$(extract_directive_ports "$DEV_CSP" img-src)"

echo "tauri.conf.json CSP ports:     $(echo "$PROD_PORTS" | tr '\n' ' ')"
echo "tauri.dev.conf.json CSP ports: $(echo "$DEV_PORTS" | tr '\n' ' ')"
echo ""

ERRORS=0

PROD_CONNECT_MISSING="$(missing_ports_in_range "$PROD_CONNECT_PORTS")"
PROD_IMG_MISSING="$(missing_ports_in_range "$PROD_IMG_PORTS")"
if [ -z "$PROD_CONNECT_MISSING" ] && [ -z "$PROD_IMG_MISSING" ]; then
  green "OK: production CSP allows local dashboard API/SSE and frame image ports."
else
  red "FAIL: production CSP must allow the local dashboard port range."
  yellow "  Missing connect-src ports:$PROD_CONNECT_MISSING"
  yellow "  Missing img-src ports:$PROD_IMG_MISSING"
  ERRORS=$((ERRORS + 1))
fi

DEV_CONNECT_MISSING="$(missing_ports_in_range "$DEV_CONNECT_PORTS")"
DEV_IMG_MISSING="$(missing_ports_in_range "$DEV_IMG_PORTS")"
if [ -z "$DEV_CONNECT_MISSING" ] && [ -z "$DEV_IMG_MISSING" ]; then
  green "OK: dev CSP allows local dashboard API/SSE and frame image ports."
else
  red "FAIL: dev CSP must allow the local dashboard port range."
  yellow "  Missing connect-src ports:$DEV_CONNECT_MISSING"
  yellow "  Missing img-src ports:$DEV_IMG_MISSING"
  ERRORS=$((ERRORS + 1))
fi

OUTSIDE_RANGE="$(ports_in_range "$ALL_PORTS")"
if [ -z "$OUTSIDE_RANGE" ]; then
  green "OK: all localhost CSP ports stay in $RUST_PORT_BASE-$RUST_PORT_END."
else
  red "FAIL: CSP contains localhost ports outside $RUST_PORT_BASE-$RUST_PORT_END."
  yellow "  Outside range:$OUTSIDE_RANGE"
  ERRORS=$((ERRORS + 1))
fi

echo ""

if [ "$ERRORS" -gt 0 ]; then
  red "=== $ERRORS check(s) FAILED ==="
  exit 1
fi

green "=== All Tauri CSP port policy checks passed ==="
