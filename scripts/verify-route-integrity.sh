#!/usr/bin/env bash
# verify-route-integrity.sh — Pre-commit hook validating route-tree.ts integrity
# Checks: i18n key coverage, defaultChild validity, Rust emit path compatibility,
#          component import existence, no orphaned pageSidebarConfig references.
set -euo pipefail

FRONTEND="crates/oneshim-web/frontend"
ROUTE_TREE="$FRONTEND/src/routes/route-tree.ts"
EN_JSON="$FRONTEND/src/i18n/locales/en.json"
KO_JSON="$FRONTEND/src/i18n/locales/ko.json"
TRAY_RS="src-tauri/src/tray.rs"
EVENT_BRIDGE="$FRONTEND/src/hooks/useTauriEventBridge.ts"

ERRORS=0

log_error() {
  echo "[route-integrity] $1"
  ERRORS=$((ERRORS + 1))
}

# --- Check 1: i18n key coverage ---
# Extract all labelKey values from route-tree.ts
LABEL_KEYS=$(grep -oE "labelKey: '[^']+'" "$ROUTE_TREE" | sed "s/labelKey: '//;s/'//")

for key in $LABEL_KEYS; do
  # Check nested key path in JSON (e.g., "nav.dashboard" → "nav" → "dashboard")
  IFS='.' read -ra PARTS <<< "$key"
  for locale_file in "$EN_JSON" "$KO_JSON"; do
    locale_name=$(basename "$locale_file")
    # Use node to check nested key existence
    FOUND=$(node -e "
      const j = require('./$locale_file');
      let v = j;
      for (const p of '${key}'.split('.')) { v = v?.[p]; }
      console.log(v !== undefined ? 'found' : 'missing');
    " 2>/dev/null || echo "missing")
    if [ "$FOUND" = "missing" ]; then
      log_error "missing i18n key: $key ($locale_name)"
    fi
  done
done

# --- Check 2: defaultChild validity ---
# Extract nodes with children and defaultChild, verify match
node -e "
  const fs = require('fs');
  const src = fs.readFileSync('$ROUTE_TREE', 'utf8');
  // Simple regex extraction of route entries with defaultChild
  const blocks = src.matchAll(/path:\s*'([^']+)'[\s\S]*?defaultChild:\s*'([^']+)'[\s\S]*?children:\s*\[([\s\S]*?)\]/g);
  for (const m of blocks) {
    const parentPath = m[1];
    const defaultChild = m[2];
    const childPaths = [...m[3].matchAll(/path:\s*'([^']+)'/g)].map(c => c[1]);
    if (!childPaths.includes(defaultChild)) {
      console.log('INVALID:' + parentPath + ':' + defaultChild + ':' + childPaths.join(','));
    }
  }
" 2>/dev/null | while IFS=: read -r _ parentPath defaultChild childPaths; do
  log_error "invalid defaultChild: $parentPath defaultChild=$defaultChild not in children [$childPaths]"
done

# --- Check 3: Rust emit path compatibility ---
# Extract navigate paths from tray.rs and event bridge
RUST_PATHS=""
if [ -f "$TRAY_RS" ]; then
  RUST_PATHS=$(grep -oE '"navigate".*"/[^"]*"' "$TRAY_RS" | grep -oE '"/[^"]*"' | tr -d '"' || true)
fi
if [ -f "$EVENT_BRIDGE" ]; then
  # Event bridge navigate calls
  BRIDGE_PATHS=$(grep -oE "navigate\(['\"][^'\"]+['\"]" "$EVENT_BRIDGE" | grep -oE "['\"/][^'\"]*" | tr -d "'" || true)
  RUST_PATHS="$RUST_PATHS $BRIDGE_PATHS"
fi

# Extract all route paths from route-tree.ts — both top-level parents and
# parent/child combinations (e.g. `/settings/ai-automation`). We use node to
# walk the routeTree structure so deep-link Rust emits (e.g. the tray menu
# navigating straight to `/settings/ai-automation`) validate against a child
# entry instead of failing because only the parent path was listed.
ROUTE_PATHS=$(node -e "
  const fs = require('fs');
  const src = fs.readFileSync('$ROUTE_TREE', 'utf8');
  const parents = [];
  // Match each top-level route entry: path + optional children block.
  const re = /\{\s*path:\s*'([^']+)'[\s\S]*?(?:children:\s*\[([\s\S]*?)\]\s*,)?[^\{]*?(?:group:|bottom:)/g;
  for (const m of src.matchAll(re)) {
    const parentPath = m[1];
    parents.push(parentPath);
    if (m[2]) {
      const childPaths = [...m[2].matchAll(/path:\s*'([^']+)'/g)].map((c) => c[1]);
      const base = parentPath === '/' ? '' : parentPath;
      for (const child of childPaths) parents.push(base + '/' + child);
    }
  }
  console.log([...new Set(parents)].join('\n'));
" 2>/dev/null | sort -u)

for rpath in $RUST_PATHS; do
  # Skip empty and non-path values
  [ -z "$rpath" ] && continue
  [[ "$rpath" != /* ]] && continue
  # Check if path exists as any known route (top-level parent or child combo)
  if ! echo "$ROUTE_PATHS" | grep -qx "$rpath"; then
    log_error "Rust emit path not in routeTree: $rpath"
  fi
done

# --- Check 4: No orphaned pageSidebarConfig references ---
if grep -rq "pageSidebarConfig" "$FRONTEND/src/" 2>/dev/null; then
  ORPHANS=$(grep -rn "pageSidebarConfig" "$FRONTEND/src/" 2>/dev/null || true)
  if [ -n "$ORPHANS" ]; then
    log_error "orphaned pageSidebarConfig reference found:\n$ORPHANS"
  fi
fi

# --- Check 5: Component import paths exist ---
# Extract lazy import paths from route-tree.ts
IMPORTS=$(grep -oE "import\(['\"][^'\"]+['\"]\)" "$ROUTE_TREE" | sed "s/import('//;s/')//;s/import(\"//;s/\")//" || true)

for imp in $IMPORTS; do
  # Resolve relative to route-tree.ts location
  RESOLVED="$FRONTEND/src/routes/$imp"
  # Try .tsx, .ts, /index.tsx, /index.ts
  FOUND=false
  for ext in ".tsx" ".ts" "/index.tsx" "/index.ts"; do
    if [ -f "${RESOLVED}${ext}" ]; then
      FOUND=true
      break
    fi
  done
  if [ "$FOUND" = "false" ]; then
    log_error "missing component: $imp (from route-tree.ts)"
  fi
done

# --- Result ---
if [ "$ERRORS" -gt 0 ]; then
  echo ""
  echo "[route-integrity] ❌ $ERRORS error(s) found"
  exit 1
else
  echo "[route-integrity] ✅ validation passed"
fi
