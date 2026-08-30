#!/usr/bin/env bash
# Read-only gate for macOS debug demo/QC profile isolation (#11618).
set -euo pipefail

FLAVOR="${MAEKON_DEV_QC_FLAVOR:-}"
PROFILE_ROOT="${MAEKON_DEV_KEYCHAIN_PROFILE_ROOT:-}"

usage() {
  echo "Usage: $0 --flavor <qc-*|tc-*>"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --flavor)
      [[ $# -ge 2 ]] || { usage >&2; exit 2; }
      FLAVOR="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "error: unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

echo "mode=read-only"
echo "keychain_queries=0"
echo "keychain_mutations=0"

if [[ ! "$FLAVOR" =~ ^(qc|tc)-[A-Za-z0-9][A-Za-z0-9_-]{2,63}$ ]]; then
  echo "result=blocked"
  echo "reason=isolated_flavor_required"
  echo "action=set MAEKON_DEV_QC_FLAVOR to a fresh qc-* or tc-* value"
  exit 2
fi

if [[ -z "$PROFILE_ROOT" ]]; then
  if [[ -z "${HOME:-}" ]]; then
    echo "result=blocked"
    echo "reason=profile_root_unavailable"
    exit 2
  fi
  PROFILE_ROOT="$HOME/Library/Application Support"
fi

LEGACY_PROFILE="$PROFILE_ROOT/maekon-dev"
TARGET_PROFILE="$PROFILE_ROOT/maekon-$FLAVOR"
LEGACY_REGISTRIES=(
  "$LEGACY_PROFILE/maekon-keychain-registry.json"
  "$LEGACY_PROFILE/data/maekon-master-key-keychain-registry.json"
)

legacy_registry_files=0
for registry in "${LEGACY_REGISTRIES[@]}"; do
  if [[ -s "$registry" ]]; then
    legacy_registry_files=$((legacy_registry_files + 1))
  fi
done

echo "requested_flavor=$FLAVOR"
echo "keychain_service=maekon-$FLAVOR"
if [[ -d "$LEGACY_PROFILE" ]]; then
  echo "legacy_dev_profile=present"
else
  echo "legacy_dev_profile=absent"
fi
echo "legacy_registry_files=$legacy_registry_files"

if [[ -e "$TARGET_PROFILE" || -L "$TARGET_PROFILE" ]]; then
  echo "result=blocked"
  echo "reason=target_profile_already_exists"
  echo "action=choose a fresh qc-* or tc-* flavor; existing profiles are never deleted"
  exit 2
fi

echo "target_profile=fresh"
echo "result=pass"
echo "guarantee=maekon-dev secrets are outside this build's service and profile namespace"
