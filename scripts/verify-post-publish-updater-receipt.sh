#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "usage: $0 <vX.Y.Z-rc.N> <receipt.json>" >&2
  exit 2
fi

TAG="$1"
RECEIPT="$2"
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd)"
REPO_ROOT="$(git -C "$SCRIPT_DIR" rev-parse --show-toplevel)"

if ! COMMIT_SHA="$(git -C "$REPO_ROOT" rev-parse --verify "refs/tags/${TAG}^{commit}" 2>/dev/null)"; then
  echo "post-publish updater receipt rejection: tag ${TAG} is not available locally" >&2
  exit 1
fi

PYTHON_BIN=""
for candidate in python3 python; do
  if command -v "$candidate" >/dev/null 2>&1; then
    PYTHON_BIN="$candidate"
    break
  fi
done
if [[ -z "$PYTHON_BIN" ]]; then
  echo "post-publish updater receipt rejection: Python is required" >&2
  exit 1
fi

if ! PREVIOUS_TAG="$(
  "$PYTHON_BIN" -c 'import json, sys; print(json.load(open(sys.argv[1], encoding="utf-8"))["previous_release_tag"])' "$RECEIPT"
)"; then
  echo "post-publish updater receipt rejection: previous_release_tag cannot be read" >&2
  exit 1
fi
if [[ ! "$PREVIOUS_TAG" =~ ^v[0-9]+\.[0-9]+\.[0-9]+-rc\.[0-9]+$ ]]; then
  echo "post-publish updater receipt rejection: previous_release_tag must use vX.Y.Z-rc.N" >&2
  exit 1
fi
if ! PREVIOUS_COMMIT_SHA="$(
  git -C "$REPO_ROOT" rev-parse --verify "refs/tags/${PREVIOUS_TAG}^{commit}" 2>/dev/null
)"; then
  echo "post-publish updater receipt rejection: previous tag ${PREVIOUS_TAG} is not available locally" >&2
  exit 1
fi

exec "$PYTHON_BIN" "$SCRIPT_DIR/post_publish_updater_receipt.py" validate \
  --receipt "$RECEIPT" \
  --release-tag "$TAG" \
  --commit-sha "$COMMIT_SHA" \
  --previous-commit-sha "$PREVIOUS_COMMIT_SHA"
