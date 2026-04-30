#!/usr/bin/env bash

set -euo pipefail

ASSETS_DIR="${ONESHIM_SMOKE_INSTALLERS_DIR:-smoke-installers}"
DMG_NAME="${ONESHIM_SMOKE_DMG_NAME:-oneshim-macos-universal.dmg}"
PKG_NAME="${ONESHIM_SMOKE_PKG_NAME:-oneshim-macos-universal.pkg}"
APP_NAME="${ONESHIM_SMOKE_APP_NAME:-Maekon.app}"

info() {
  printf '[INSTALLER-SMOKE] %s\n' "$*"
}

fatal() {
  printf '[INSTALLER-SMOKE][ERROR] %s\n' "$*" >&2
  exit 1
}

usage() {
  cat <<'EOF'
macOS installer smoke test for release artifacts

Usage:
  ./scripts/release-installer-smoke-macos.sh [options]

Options:
  --assets-dir <path>  Directory containing DMG/PKG artifacts
  --dmg-name <name>    DMG file name (default: oneshim-macos-universal.dmg)
  --pkg-name <name>    PKG file name (default: oneshim-macos-universal.pkg)
  --app-name <name>    Installed app bundle name (default: Maekon.app)
  -h, --help           Show help
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --assets-dir)
      [[ $# -ge 2 ]] || fatal "--assets-dir requires a value"
      ASSETS_DIR="$2"
      shift 2
      ;;
    --dmg-name)
      [[ $# -ge 2 ]] || fatal "--dmg-name requires a value"
      DMG_NAME="$2"
      shift 2
      ;;
    --pkg-name)
      [[ $# -ge 2 ]] || fatal "--pkg-name requires a value"
      PKG_NAME="$2"
      shift 2
      ;;
    --app-name)
      [[ $# -ge 2 ]] || fatal "--app-name requires a value"
      APP_NAME="$2"
      shift 2
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      fatal "Unknown option: $1"
      ;;
  esac
done

[[ -d "$ASSETS_DIR" ]] || fatal "Asset directory not found: $ASSETS_DIR"

resolve_asset() {
  local file_name="$1"
  local direct_path="$ASSETS_DIR/$file_name"
  if [[ -f "$direct_path" ]]; then
    printf '%s' "$direct_path"
    return
  fi

  local found_path
  found_path="$(find "$ASSETS_DIR" -type f -name "$file_name" | head -n1 || true)"
  if [[ -z "$found_path" ]]; then
    fatal "Asset not found: $file_name"
  fi
  printf '%s' "$found_path"
}

run_as_root() {
  if command -v sudo >/dev/null 2>&1; then
    sudo "$@"
  else
    "$@"
  fi
}

validate_binary_format() {
  local bin_path="$1"
  local file_info
  file_info="$(file "$bin_path")"
  info "Binary format: $file_info"
  if ! echo "$file_info" | grep -qE "Mach-O|universal binary"; then
    fatal "Binary is not a valid macOS executable: $file_info"
  fi
}

run_gui_bootstrap_smoke() {
  local bin_path="$1"
  local label="$2"
  local log_path="$LOG_DIR/gui-bootstrap-${label}.log"

  info "Running GUI bootstrap smoke (${label})"

  # Tauri is a GUI app — launch briefly and check for panics.
  # On headless CI, display/WebKit failures are expected and non-fatal.
  set +e
  ONESHIM_DISABLE_TRAY=1 "$bin_path" >"$log_path" 2>&1 &
  local pid=$!
  sleep 3
  if kill -0 "$pid" >/dev/null 2>&1; then
    # Tauri may block on WindowServer/WKWebView init on headless macOS CI
    # and ignore SIGTERM long enough for wait() to hang indefinitely.
    kill "$pid" >/dev/null 2>&1 || true
    local grace=0
    while kill -0 "$pid" >/dev/null 2>&1 && [[ "$grace" -lt 5 ]]; do
      sleep 1
      grace=$((grace + 1))
    done
    if kill -0 "$pid" >/dev/null 2>&1; then
      info "SIGTERM did not terminate process $pid after 5s; sending SIGKILL"
      kill -9 "$pid" >/dev/null 2>&1 || true
    fi
  fi
  wait "$pid" 2>/dev/null
  local rc=$?
  set -e

  # Fatal: Rust panics or tokio runtime double-init
  if grep -qE "Cannot start a runtime from within a runtime|Cannot drop a runtime|panicked at|SIGABRT|stack backtrace" "$log_path"; then
    if ! grep -qE "WKWebView|no display|NSApplication" "$log_path" || \
       grep -qE "Cannot start a runtime|Cannot drop a runtime|SIGABRT|stack backtrace" "$log_path"; then
      cat "$log_path"
      fatal "GUI bootstrap smoke detected runtime/panic failure (${label})"
    fi
  fi

  # Only flag segfault/abort crashes
  if [[ "$rc" -eq 139 || "$rc" -eq 134 ]]; then
    cat "$log_path"
    fatal "GUI bootstrap smoke crashed (rc=$rc, ${label})"
  fi

  info "GUI bootstrap smoke OK (rc=$rc, ${label})"
}

find_existing_app_path() {
  local candidate
  for candidate in "$SYSTEM_APP_INSTALL_PATH" "$USER_APP_INSTALL_PATH"; do
    if [[ -d "$candidate" ]]; then
      printf '%s' "$candidate"
      return 0
    fi
  done
  return 1
}

find_installed_app_path() {
  local candidate
  for candidate in "$SYSTEM_APP_INSTALL_PATH" "$USER_APP_INSTALL_PATH"; do
    if [[ -x "$candidate/Contents/MacOS/oneshim" ]]; then
      printf '%s' "$candidate"
      return 0
    fi
  done

  local found_path
  found_path="$(find /Applications "$HOME/Applications" -maxdepth 1 -type d -name "$APP_NAME" 2>/dev/null | head -n1 || true)"
  if [[ -n "$found_path" && -x "$found_path/Contents/MacOS/oneshim" ]]; then
    printf '%s' "$found_path"
    return 0
  fi

  return 1
}

DMG_PATH="$(resolve_asset "$DMG_NAME")"
PKG_PATH="$(resolve_asset "$PKG_NAME")"

TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/oneshim-installer-smoke.XXXXXX")"
MOUNT_DIR="$TMP_DIR/mount"
LOG_DIR="${ONESHIM_SMOKE_LOG_DIR:-${RUNNER_TEMP:-$TMP_DIR}/oneshim-installer-smoke}"
SYSTEM_APP_INSTALL_PATH="/Applications/$APP_NAME"
USER_APP_INSTALL_PATH="${HOME}/Applications/$APP_NAME"
APP_INSTALL_PATH="$SYSTEM_APP_INSTALL_PATH"
APP_BINARY_PATH="$APP_INSTALL_PATH/Contents/MacOS/oneshim"
APP_BACKUP_PATH="$TMP_DIR/${APP_NAME}.backup"
MOUNTED=0
APP_WAS_PRESENT=0
APP_RESTORE_PATH=""

mkdir -p "$LOG_DIR"

cleanup() {
  if [[ "$MOUNTED" == "1" ]]; then
    hdiutil detach "$MOUNT_DIR" -quiet || true
  fi

  local candidate
  for candidate in "$SYSTEM_APP_INSTALL_PATH" "$USER_APP_INSTALL_PATH"; do
    if [[ "$APP_WAS_PRESENT" == "1" && "$candidate" == "$APP_RESTORE_PATH" ]]; then
      continue
    fi
    if [[ -d "$candidate" ]]; then
      run_as_root rm -rf "$candidate" || true
    fi
  done

  if [[ "$APP_WAS_PRESENT" == "1" && -n "$APP_RESTORE_PATH" && -d "$APP_BACKUP_PATH" ]]; then
    run_as_root mv "$APP_BACKUP_PATH" "$APP_RESTORE_PATH" || true
  fi

  rm -rf "$TMP_DIR"
}
trap cleanup EXIT

info "Using DMG: $DMG_PATH"
info "Using PKG: $PKG_PATH"

EXISTING_APP_PATH="$(find_existing_app_path || true)"
if [[ -n "$EXISTING_APP_PATH" ]]; then
  info "Backing up existing app at $EXISTING_APP_PATH"
  run_as_root mv "$EXISTING_APP_PATH" "$APP_BACKUP_PATH"
  APP_WAS_PRESENT=1
  APP_RESTORE_PATH="$EXISTING_APP_PATH"
fi

mkdir -p "$MOUNT_DIR"
info "Mounting DMG"
hdiutil attach "$DMG_PATH" -nobrowse -readonly -mountpoint "$MOUNT_DIR" -quiet
MOUNTED=1

DMG_APP_PATH="$MOUNT_DIR/$APP_NAME"
DMG_BINARY_PATH="$DMG_APP_PATH/Contents/MacOS/oneshim"
DMG_SIDECAR_PATH="$DMG_APP_PATH/Contents/MacOS/oneshim-sandbox-worker"
[[ -d "$DMG_APP_PATH" ]] || fatal "App bundle missing in DMG: $DMG_APP_PATH"
[[ -x "$DMG_BINARY_PATH" ]] || fatal "Binary missing in DMG app bundle: $DMG_BINARY_PATH"
[[ -x "$DMG_SIDECAR_PATH" ]] || fatal "Sandbox worker sidecar missing in DMG app bundle: $DMG_SIDECAR_PATH"

info "Validating binary from mounted DMG"
validate_binary_format "$DMG_BINARY_PATH"
validate_binary_format "$DMG_SIDECAR_PATH"
run_gui_bootstrap_smoke "$DMG_BINARY_PATH" "dmg"

info "Installing PKG"
run_as_root installer -pkg "$PKG_PATH" -target / >/dev/null
APP_INSTALL_PATH="$(find_installed_app_path || true)"
[[ -n "$APP_INSTALL_PATH" ]] || fatal "Installed app bundle missing under /Applications or ~/Applications"
APP_BINARY_PATH="$APP_INSTALL_PATH/Contents/MacOS/oneshim"
APP_SIDECAR_PATH="$APP_INSTALL_PATH/Contents/MacOS/oneshim-sandbox-worker"
[[ -x "$APP_BINARY_PATH" ]] || fatal "Installed app binary missing: $APP_BINARY_PATH"
[[ -x "$APP_SIDECAR_PATH" ]] || fatal "Installed sandbox worker sidecar missing: $APP_SIDECAR_PATH"
info "Detected installed app at $APP_INSTALL_PATH"

info "Validating binary from PKG installation"
validate_binary_format "$APP_BINARY_PATH"
validate_binary_format "$APP_SIDECAR_PATH"
run_gui_bootstrap_smoke "$APP_BINARY_PATH" "pkg"

info "macOS installer smoke completed"
