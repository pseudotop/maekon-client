#!/usr/bin/env bash
# Verify that the macOS assets published for one or more release tags are
# notarized and stapled.
#
# This check is deliberately independent of notarize-macos-release-assets.yml.
# It inspects the bytes a user actually downloads from the release page, so it
# reports the same failure whether the notarization workflow failed, was
# cancelled, or never ran at all (#10935).
#
# v0.0.1-rc.8 shipped signed-but-unnotarized macOS assets for ~13 hours because
# the notarization workflow failed on its own and the Release workflow stayed
# green. Nothing on the release page distinguished the two states.
#
# Usage:
#   scripts/verify-published-macos-notarization.sh <tag> [<tag>...]
#   scripts/verify-published-macos-notarization.sh --latest [count]
set -euo pipefail

REPO="${MAEKON_PUBLIC_REPO:-pseudotop/maekon-client}"
MACOS_ASSETS=(maekon-macos-universal.dmg maekon-macos-universal.pkg)

die() {
  echo "error: $*" >&2
  exit 1
}

command -v gh >/dev/null 2>&1 || die "gh is required"
command -v xcrun >/dev/null 2>&1 || die "xcrun is required; run this on macOS"

resolve_latest_tags() {
  local count="$1"
  gh release list --repo "$REPO" --limit "$count" --json tagName --jq '.[].tagName'
}

# Returns 0 when every macOS asset for the tag is stapled, 1 otherwise. A tag
# that publishes no macOS assets is reported as a failure rather than skipped:
# a release that silently lost its DMG/PKG is exactly the condition worth
# catching, and treating "absent" as "fine" would hide it.
verify_tag() {
  local tag="$1"
  local workdir status asset
  workdir="$(mktemp -d)"
  status=0

  echo "== ${tag} =="

  if ! gh release download "$tag" \
    --repo "$REPO" \
    --dir "$workdir" \
    --pattern 'maekon-macos-universal.dmg' \
    --pattern 'maekon-macos-universal.pkg' >/dev/null 2>&1; then
    echo "  FAIL: could not download macOS assets for ${tag}"
    rm -rf "$workdir"
    return 1
  fi

  for asset in "${MACOS_ASSETS[@]}"; do
    if [ ! -f "${workdir}/${asset}" ]; then
      echo "  FAIL: ${asset} is not published for ${tag}"
      status=1
      continue
    fi

    if xcrun stapler validate "${workdir}/${asset}" >/dev/null 2>&1; then
      echo "  ok:   ${asset} is stapled"
    else
      echo "  FAIL: ${asset} is NOT stapled — Gatekeeper will refuse it"
      status=1
    fi
  done

  # The PKG is what a user double-clicks, so assess it the way Gatekeeper will.
  # Skipped when the PKG itself is missing or unstapled, since the stapler
  # result above is already the actionable failure.
  if [ "$status" -eq 0 ]; then
    if spctl --assess --type install "${workdir}/maekon-macos-universal.pkg" >/dev/null 2>&1; then
      echo "  ok:   maekon-macos-universal.pkg accepted by Gatekeeper"
    else
      echo "  FAIL: maekon-macos-universal.pkg rejected by Gatekeeper"
      status=1
    fi
  fi

  rm -rf "$workdir"
  return "$status"
}

TAGS=()
if [ "${1:-}" = "--latest" ]; then
  count="${2:-5}"
  while IFS= read -r tag; do
    [ -n "$tag" ] && TAGS+=("$tag")
  done < <(resolve_latest_tags "$count")
  [ "${#TAGS[@]}" -gt 0 ] || die "no releases found in ${REPO}"
elif [ "$#" -gt 0 ]; then
  TAGS=("$@")
else
  die "usage: $0 <tag> [<tag>...] | --latest [count]"
fi

overall=0
for tag in "${TAGS[@]}"; do
  verify_tag "$tag" || overall=1
done

if [ "$overall" -ne 0 ]; then
  echo
  echo "::error::One or more published releases carry unstapled macOS assets."
  exit 1
fi

echo
echo "All checked releases publish stapled macOS assets."
