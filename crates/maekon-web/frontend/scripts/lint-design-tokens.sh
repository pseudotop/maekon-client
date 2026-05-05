#!/usr/bin/env bash
set -euo pipefail

# Design System Token Lint — CI Required Gate
# Scans components for forbidden patterns that bypass the design token system.
# Exit 0 = clean, Exit 1 = violations found.

FRONTEND_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SRC_DIRS=("$FRONTEND_DIR/src/components" "$FRONTEND_DIR/src/pages" "$FRONTEND_DIR/src/overlay")
# Exclusions:
#   *.test.tsx / *.stories.tsx — test/story files
#   DevToolbar.tsx — dev-only toolbar
#   DetectionHeader.tsx / DetectionOverlay.tsx — overlay Tauri windows with own CSS (no design tokens)
#   tracking-panel/ — separate Tauri panel window with own CSS (no design tokens)
EXCLUDE="--exclude=*.test.tsx --exclude=*.stories.tsx --exclude=DevToolbar.tsx --exclude=DetectionHeader.tsx --exclude=DetectionOverlay.tsx --exclude-dir=tracking-panel"

ERRORS=0

check_pattern() {
  local label="$1"
  shift
  local result
  result=$(grep -rn $EXCLUDE "$@" "${SRC_DIRS[@]}" 2>/dev/null || true)
  if [[ -n "$result" ]]; then
    echo "=== $label ==="
    echo "$result"
    echo ""
    ERRORS=$((ERRORS + 1))
  fi
}

echo "Design Token Lint: scanning components..."
echo ""

# Hardcoded colors (include neutral)
check_pattern "Hardcoded color: text-{color}-N" \
  -E "text-(teal|blue|purple|green|amber|red|orange|pink|indigo|emerald|violet|neutral)-[0-9]"

check_pattern "Hardcoded color: bg-{color}-N" \
  -E "bg-(teal|blue|purple|green|amber|red|orange|pink|indigo|emerald|violet|neutral)-[0-9]"

check_pattern "Hardcoded color: border-{color}-N" \
  -E "border-(teal|blue|purple|green|amber|red|orange|pink|indigo|emerald|violet|neutral)-[0-9]"

check_pattern "Hardcoded gradient: from/to-{color}" \
  -E "(from|to)-(teal|blue|purple|green|amber|red|orange)-[0-9]"

# Hardcoded white/black (use content-inverse/surface-sunken)
check_pattern "Hardcoded text-white/bg-black" \
  -E "\b(text-white|bg-black)\b"

# Hardcoded typography
check_pattern "Hardcoded font weight: font-bold/semibold/medium/mono" \
  -E "font-(bold|semibold|medium|mono)"

# Bare transitions
check_pattern "Bare transition class" \
  -E "transition-(colors|all|opacity|transform)"

# dark: prefix
check_pattern "dark: prefix (use CSS vars)" \
  -E "dark:(bg|text|border|from|to)-"

# Off-scale spacing (precise prefixes only)
check_pattern "Off-scale spacing (use {0,1,2,3,4,6,8,12} only)" \
  -E "\b(gap|space-[xy]|p|px|py|pt|pb|pl|pr|m|mx|my|mt|mb|ml|mr)-(5|7|9|10|11|13|14|15)\b"

check_pattern "Arbitrary spacing value" \
  -E "(gap|space-[xy]|p[xytblr]?|m[xytblr]?)-\[.+\]"

# Inline icon sizes (should use iconSize.* tokens)
# Match paired w-N h-N or h-N w-N (N=3..5) but NOT fractional widths (e.g. w-3/4)
check_pattern_icon_size() {
  local label="$1"
  local result
  result=$(grep -rn $EXCLUDE -E "\bw-[345]\s+h-[345]\b|\bh-[345]\s+w-[345]\b" "${SRC_DIRS[@]}" 2>/dev/null \
    | grep -v "w-[0-9]/[0-9]" || true)
  if [[ -n "$result" ]]; then
    echo "=== $label ==="
    echo "$result"
    echo ""
    ERRORS=$((ERRORS + 1))
  fi
}
check_pattern_icon_size "Inline icon size (use iconSize.* tokens)"

if [[ $ERRORS -gt 0 ]]; then
  echo "FAIL: $ERRORS violation categories found. Fix all above patterns."
  exit 1
else
  echo "PASS: No design token violations found."
  exit 0
fi
