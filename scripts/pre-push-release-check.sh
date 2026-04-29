#!/usr/bin/env bash
set -euo pipefail

source ./scripts/release-common.sh

timeout_secs="${ONESHIM_PRE_PUSH_STDIN_TIMEOUT_SECS:-1}"

while IFS=' ' read -r -t "$timeout_secs" local_ref local_sha remote_ref remote_sha; do
  case "$remote_ref" in
    refs/tags/v*)
      TAG_VERSION="${remote_ref#refs/tags/v}"
      echo "🏷️  Tag push detected: v${TAG_VERSION}"
      echo ""
      ./scripts/pre-release-check.sh "$TAG_VERSION"
      ;;
    refs/heads/release/v*-rc.*)
      # Extract version from branch name: release/v0.4.2-rc.1 -> 0.4.2-rc.1.
      BRANCH_VERSION="${remote_ref#refs/heads/release/v}"
      BASE="$(base_version "$BRANCH_VERSION")"
      if git tag -l "v${BASE}" | grep -q "^v${BASE}$"; then
        echo "❌ release branch v${BRANCH_VERSION}: stable tag v${BASE} already exists"
        echo "   Hint: use next patch version (e.g. v$(next_patch_version "$BASE")-rc.1)"
        exit 1
      fi
      echo "✅ release branch v${BRANCH_VERSION}: no conflicting stable tag"
      ;;
  esac
done
