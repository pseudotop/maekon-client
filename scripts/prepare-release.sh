#!/usr/bin/env bash
# prepare-release.sh — Deprecated wrapper kept to block direct stable releases.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
source "$ROOT_DIR/scripts/release-common.sh"

VERSION="${1:-}"

echo "prepare-release.sh is deprecated."
echo "Use one of the enforced paths instead:"
echo "  RC publish:      ./scripts/release.sh <x.y.z-rc.N>"
echo "  Stable promote:  ./scripts/promote-stable.sh <x.y.z-rc.N>"

if [ -n "$VERSION" ] && is_stable_version "$VERSION"; then
  echo ""
  echo "Direct stable preparation is disabled. Promote the latest validated RC instead."
fi

exit 1
