#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
TARGET_DIR="${ONESHIM_TARGET_DIR:-$PROJECT_ROOT/target}"
TARGET_SOFT_LIMIT_MB="${ONESHIM_TARGET_SOFT_LIMIT_MB:-8192}"
TARGET_HARD_LIMIT_MB="${ONESHIM_TARGET_HARD_LIMIT_MB:-12288}"
TARGET_AUTO_PRUNE="${ONESHIM_TARGET_AUTO_PRUNE:-1}"

if ! [[ "$TARGET_SOFT_LIMIT_MB" =~ ^[0-9]+$ ]]; then
  echo "Error: ONESHIM_TARGET_SOFT_LIMIT_MB must be a positive integer."
  exit 1
fi

if ! [[ "$TARGET_HARD_LIMIT_MB" =~ ^[0-9]+$ ]]; then
  echo "Error: ONESHIM_TARGET_HARD_LIMIT_MB must be a positive integer."
  exit 1
fi

if [ "$TARGET_HARD_LIMIT_MB" -lt "$TARGET_SOFT_LIMIT_MB" ]; then
  echo "Error: ONESHIM_TARGET_HARD_LIMIT_MB must be greater than or equal to ONESHIM_TARGET_SOFT_LIMIT_MB."
  exit 1
fi

target_size_kb() {
  if [ ! -d "$TARGET_DIR" ]; then
    echo "0"
    return
  fi
  du -sk "$TARGET_DIR" 2>/dev/null | awk '{print $1}'
}

prune_target_if_needed() {
  if [ "$TARGET_AUTO_PRUNE" != "1" ] || [ ! -d "$TARGET_DIR" ]; then
    return
  fi

  local soft_limit_kb=$((TARGET_SOFT_LIMIT_MB * 1024))
  local hard_limit_kb=$((TARGET_HARD_LIMIT_MB * 1024))
  local current_size_kb

  current_size_kb="$(target_size_kb)"
  if [ "$current_size_kb" -le "$soft_limit_kb" ]; then
    return
  fi

  echo "[cargo-cache] target directory exceeds soft limit (${TARGET_SOFT_LIMIT_MB}MB)."
  echo "[cargo-cache] pruning incremental cache: $TARGET_DIR/debug/incremental"
  rm -rf "$TARGET_DIR/debug/incremental"
  current_size_kb="$(target_size_kb)"

  if [ "$current_size_kb" -le "$soft_limit_kb" ]; then
    return
  fi

  echo "[cargo-cache] target still above soft limit. pruning dep artifacts: $TARGET_DIR/debug/deps"
  rm -rf "$TARGET_DIR/debug/deps"
  current_size_kb="$(target_size_kb)"

  if [ "$current_size_kb" -le "$hard_limit_kb" ]; then
    return
  fi

  echo "[cargo-cache] target exceeds hard limit (${TARGET_HARD_LIMIT_MB}MB). pruning build script outputs: $TARGET_DIR/debug/build"
  rm -rf "$TARGET_DIR/debug/build"
  current_size_kb="$(target_size_kb)"

  if [ "$current_size_kb" -gt "$hard_limit_kb" ]; then
    echo "[cargo-cache] target is still large (${current_size_kb}KB). run 'cargo clean' if you need full cleanup."
  fi
}

print_target_status() {
  local current_size_kb
  current_size_kb="$(target_size_kb)"
  local current_size_mb=$((current_size_kb / 1024))
  echo "[cargo-cache] target path: $TARGET_DIR"
  echo "[cargo-cache] target size: ${current_size_mb}MB (${current_size_kb}KB)"
  echo "[cargo-cache] soft/hard limit: ${TARGET_SOFT_LIMIT_MB}MB / ${TARGET_HARD_LIMIT_MB}MB"
  echo "[cargo-cache] auto prune: $TARGET_AUTO_PRUNE"
}

if [ $# -eq 0 ]; then
  echo "Usage:"
  echo "  ./scripts/cargo-cache.sh <cargo-args...>"
  echo "  ./scripts/cargo-cache.sh --status"
  echo ""
  echo "Environment variables:"
  echo "  ONESHIM_TARGET_AUTO_PRUNE (default: 1)"
  echo "  ONESHIM_TARGET_SOFT_LIMIT_MB (default: 8192)"
  echo "  ONESHIM_TARGET_HARD_LIMIT_MB (default: 12288)"
  echo "  ONESHIM_TARGET_DIR (default: <repo>/target)"
  exit 1
fi

if [ "${1:-}" = "--status" ]; then
  print_target_status
  exit 0
fi

if command -v sccache >/dev/null 2>&1; then
  export RUSTC_WRAPPER="$(command -v sccache)"
  export SCCACHE_CACHE_SIZE="${SCCACHE_CACHE_SIZE:-6G}"
  sccache --start-server >/dev/null 2>&1 || true
fi

prune_target_if_needed

set +e
cargo "$@"
status=$?
set -e

prune_target_if_needed

exit "$status"
