#!/usr/bin/env bash

set -euo pipefail

EVENT_NAME="${EVENT_NAME:-${1:-${GITHUB_EVENT_NAME:-}}}"
BASE_SHA="${BASE_SHA:-${2:-}}"
HEAD_SHA="${HEAD_SHA:-${3:-$(git rev-parse HEAD)}}"
OUTPUT_PATH="${GITHUB_OUTPUT:-}"

if [[ -z "$OUTPUT_PATH" ]]; then
  echo "GITHUB_OUTPUT is required" >&2
  exit 1
fi

emit_all_true() {
  {
    echo "rust=true"
    echo "frontend=true"
    echo "ci=true"
    echo "release_metadata_only=false"
  } >> "$OUTPUT_PATH"
}

if [[ "${EVENT_NAME}" == "workflow_dispatch" ]]; then
  echo "workflow_dispatch requested; enabling all change flags"
  emit_all_true
  exit 0
fi

if [[ -z "${BASE_SHA}" || "${BASE_SHA}" =~ ^0+$ ]]; then
  echo "No usable base SHA found; enabling all change flags"
  emit_all_true
  exit 0
fi

echo "Diff base: ${BASE_SHA}"
echo "Diff head: ${HEAD_SHA}"

mapfile -t changed_files < <(git diff --name-only "${BASE_SHA}" "${HEAD_SHA}")

if [[ "${#changed_files[@]}" -eq 0 ]]; then
  echo "No changed files detected"
fi

rust=false
frontend=false
ci=false
release_metadata_only=true
release_metadata_file_count=0

for file in "${changed_files[@]}"; do
  [[ -z "$file" ]] && continue
  echo "changed: $file"

  case "$file" in
    CHANGELOG.md|Cargo.toml|Cargo.lock|crates/oneshim-web/frontend/package.json)
      release_metadata_file_count=$((release_metadata_file_count + 1))
      ;;
    *)
      release_metadata_only=false
      ;;
  esac

  case "$file" in
    crates/oneshim-web/frontend/*)
      frontend=true
      ;;
  esac

  case "$file" in
    .github/workflows/*|.github/actions/*)
      ci=true
      ;;
  esac

  case "$file" in
    Cargo.toml|Cargo.lock|src-tauri/*|scripts/*|debian/*)
      rust=true
      ;;
    crates/*)
      if [[ "$file" != crates/oneshim-web/frontend/* ]]; then
        rust=true
      fi
      ;;
  esac
done

if [[ "${release_metadata_file_count}" -eq 0 ]]; then
  release_metadata_only=false
fi

{
  echo "rust=${rust}"
  echo "frontend=${frontend}"
  echo "ci=${ci}"
  echo "release_metadata_only=${release_metadata_only}"
} >> "$OUTPUT_PATH"
