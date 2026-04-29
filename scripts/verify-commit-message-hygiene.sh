#!/usr/bin/env bash
set -euo pipefail

BASE_SHA="${1:-}"
HEAD_SHA="${2:-HEAD}"

if [[ -n "$BASE_SHA" ]]; then
  if ! git cat-file -e "${BASE_SHA}^{commit}" 2>/dev/null; then
    git fetch --no-tags --depth=1 origin "$BASE_SHA"
  fi
  RANGE="${BASE_SHA}..${HEAD_SHA}"
elif git rev-parse --verify HEAD~1 >/dev/null 2>&1; then
  RANGE="HEAD~1..${HEAD_SHA}"
else
  echo "[commit-hygiene] single-commit context; nothing to validate"
  exit 0
fi

mapfile -t subjects < <(git log --format='%H%x09%s' "$RANGE")
if [[ ${#subjects[@]} -eq 0 ]]; then
  echo "[commit-hygiene] no commits found in range: $RANGE"
  exit 0
fi

bad=0
shopt -s nocasematch
for entry in "${subjects[@]}"; do
  sha="${entry%%$'\t'*}"
  subject="${entry#*$'\t'}"

  if [[ "$subject" =~ (api[ _-]?key|secret|password|private[ _-]?key|notary[ _-]?app[ _-]?password|apple[ _-]?id|doppler|token=|p12) ]]; then
    echo "[commit-hygiene] sensitive keyword in commit subject: $sha $subject" >&2
    bad=1
  fi

  if [[ "$subject" =~ (ONESHIM_[A-Z0-9_]+=|MACOS_[A-Z0-9_]+=) ]]; then
    echo "[commit-hygiene] raw env assignment in commit subject: $sha $subject" >&2
    bad=1
  fi
done
shopt -u nocasematch

if [[ "$bad" -ne 0 ]]; then
  echo "[commit-hygiene] failed; use squash + abstract commit subjects for sensitive topics" >&2
  exit 1
fi

echo "[commit-hygiene] validation passed"
