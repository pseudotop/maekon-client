#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
TARGET_SOFT_LIMIT_MB="${MAEKON_TARGET_SOFT_LIMIT_MB:-8192}"
TARGET_HARD_LIMIT_MB="${MAEKON_TARGET_HARD_LIMIT_MB:-12288}"
TARGET_AUTO_PRUNE="${MAEKON_TARGET_AUTO_PRUNE:-1}"

if [ -n "${CARGO_TARGET_DIR:-}" ] && [ -n "${MAEKON_TARGET_DIR:-}" ] && [ "$CARGO_TARGET_DIR" != "$MAEKON_TARGET_DIR" ]; then
  echo "Error: CARGO_TARGET_DIR and MAEKON_TARGET_DIR disagree." >&2
  echo "Set only CARGO_TARGET_DIR, or set both to the same path." >&2
  exit 1
fi

TARGET_DIR="${CARGO_TARGET_DIR:-${MAEKON_TARGET_DIR:-$PROJECT_ROOT/target}}"
export CARGO_TARGET_DIR="$TARGET_DIR"

if ! [[ "$TARGET_SOFT_LIMIT_MB" =~ ^[0-9]+$ ]]; then
  echo "Error: MAEKON_TARGET_SOFT_LIMIT_MB must be a positive integer."
  exit 1
fi

if ! [[ "$TARGET_HARD_LIMIT_MB" =~ ^[0-9]+$ ]]; then
  echo "Error: MAEKON_TARGET_HARD_LIMIT_MB must be a positive integer."
  exit 1
fi

if [ "$TARGET_HARD_LIMIT_MB" -lt "$TARGET_SOFT_LIMIT_MB" ]; then
  echo "Error: MAEKON_TARGET_HARD_LIMIT_MB must be greater than or equal to MAEKON_TARGET_SOFT_LIMIT_MB."
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
  if [ "$TARGET_AUTO_PRUNE" != "1" ]; then
    return
  fi

  # Skip only when target is genuinely absent; symlinks must still reach the safety check.
  if [ ! -L "$TARGET_DIR" ] && [ ! -e "$TARGET_DIR" ]; then
    return
  fi

  local python_bin="${MAEKON_CARGO_CACHE_PYTHON:-python3}"
  python_bin="$(command -v "$python_bin" 2>/dev/null || true)"
  if [ -z "$python_bin" ] || [ ! -x "$python_bin" ]; then
    echo "[cargo-cache] auto-prune skipped: python3 is unavailable; nothing was deleted." >&2
    return
  fi

  local disk_helper="$SCRIPT_DIR/cargo-target-disk.py"
  if [ ! -r "$disk_helper" ]; then
    echo "[cargo-cache] auto-prune skipped: helper $disk_helper is missing; nothing was deleted." >&2
    return
  fi

  local prune_status=0
  set +e
  "$python_bin" "$disk_helper" auto-prune \
    --target "$TARGET_DIR" \
    --soft-limit-mb "$TARGET_SOFT_LIMIT_MB" \
    --hard-limit-mb "$TARGET_HARD_LIMIT_MB"
  prune_status=$?
  set -e

  case "$prune_status" in
    0)
      return
      ;;
    3)
      echo "[cargo-cache] skipping auto-prune while Cargo holds the target lock." >&2
      return
      ;;
    *)
      echo "[cargo-cache] auto-prune aborted due to lock/path safety error (exit ${prune_status}); nothing was deleted." >&2
      return
      ;;
  esac
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
  echo "  MAEKON_TARGET_AUTO_PRUNE (default: 1)"
  echo "  MAEKON_TARGET_SOFT_LIMIT_MB (default: 8192)"
  echo "  MAEKON_TARGET_HARD_LIMIT_MB (default: 12288)"
  echo "  CARGO_TARGET_DIR (default: <repo>/target)"
  echo "  MAEKON_TARGET_DIR (legacy alias for CARGO_TARGET_DIR)"
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
