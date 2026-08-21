#!/usr/bin/env bash

set -euo pipefail

ASSETS_DIR="${MAEKON_SMOKE_INSTALLERS_DIR:-smoke-installers}"
DMG_NAME="${MAEKON_SMOKE_DMG_NAME:-maekon-macos-universal.dmg}"
PKG_NAME="${MAEKON_SMOKE_PKG_NAME:-maekon-macos-universal.pkg}"
APP_NAME="${MAEKON_SMOKE_APP_NAME:-Maekon.app}"
DRY_RUN_BACKUP_RESTORE_PROOF=0
SYSTEM_APPLICATIONS_DIR="${MAEKON_SYSTEM_APPLICATIONS_DIR:-/Applications}"
USER_APPLICATIONS_DIR="${MAEKON_USER_APPLICATIONS_DIR:-${HOME}/Applications}"

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
  --dmg-name <name>    DMG file name (default: maekon-macos-universal.dmg)
  --pkg-name <name>    PKG file name (default: maekon-macos-universal.pkg)
  --app-name <name>    Installed app bundle name (default: Maekon.app)
  --dry-run-backup-restore-proof
                       Exercise backup/cleanup/restore logic without DMG/PKG artifacts
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
    --dry-run-backup-restore-proof)
      DRY_RUN_BACKUP_RESTORE_PROOF=1
      shift
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
  if [[ "${MAEKON_SMOKE_NO_SUDO:-0}" == "1" ]]; then
    "$@"
    return
  fi
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
  MAEKON_DISABLE_TRAY=1 "$bin_path" >"$log_path" 2>&1 &
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

backup_existing_apps() {
  local candidate
  local index=0
  mkdir -p "$APP_BACKUP_DIR"
  for candidate in "${APP_CANDIDATE_PATHS[@]}"; do
    if [[ -d "$candidate" ]]; then
      local backup_path="$APP_BACKUP_DIR/app-$index"
      info "Backing up existing app at $candidate"
      run_as_root mv "$candidate" "$backup_path"
      APP_RESTORE_PATHS+=("$candidate")
      APP_BACKUP_PATHS+=("$backup_path")
    fi
    index=$((index + 1))
  done
}

find_installed_app_path() {
  local candidate
  for candidate in "${APP_CANDIDATE_PATHS[@]}"; do
    if [[ -x "$candidate/Contents/MacOS/maekon" ]]; then
      printf '%s' "$candidate"
      return 0
    fi
  done

  local found_path
  found_path="$(find "$SYSTEM_APPLICATIONS_DIR" "$USER_APPLICATIONS_DIR" -maxdepth 1 -type d -name "$APP_NAME" 2>/dev/null | head -n1 || true)"
  if [[ -n "$found_path" && -x "$found_path/Contents/MacOS/maekon" ]]; then
    printf '%s' "$found_path"
    return 0
  fi

  return 1
}

TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/maekon-installer-smoke.XXXXXX")"
touch "$TMP_DIR/.maekon-tmp-owned"
printf '%s\n' "$$" > "$TMP_DIR/.maekon-tmp-owner-pid"
MOUNT_DIR="$TMP_DIR/mount"
LOG_DIR="${MAEKON_SMOKE_LOG_DIR:-${RUNNER_TEMP:-$TMP_DIR}/maekon-installer-smoke}"
SYSTEM_APP_INSTALL_PATH="$SYSTEM_APPLICATIONS_DIR/$APP_NAME"
USER_APP_INSTALL_PATH="$USER_APPLICATIONS_DIR/$APP_NAME"
APP_INSTALL_PATH="$SYSTEM_APP_INSTALL_PATH"
APP_BINARY_PATH="$APP_INSTALL_PATH/Contents/MacOS/maekon"
APP_BACKUP_DIR="$TMP_DIR/app-backups"
MOUNTED=0
declare -a APP_CANDIDATE_PATHS=("$SYSTEM_APP_INSTALL_PATH" "$USER_APP_INSTALL_PATH")
declare -a APP_BACKUP_PATHS=()
declare -a APP_RESTORE_PATHS=()

mkdir -p "$LOG_DIR"

detach_mount() {
  # A GUI bootstrap smoke launched from the DMG can still hold the volume when
  # cleanup runs, and diskimages-helper outlives the process that opened it.
  # One quiet detach loses that race, and `|| true` swallowed the failure — so
  # the volume stayed mounted and every later path was read-only.
  local attempt
  for attempt in 1 2 3; do
    if hdiutil detach "$MOUNT_DIR" -quiet; then
      return 0
    fi
    sleep 2
  done
  if hdiutil detach "$MOUNT_DIR" -force -quiet; then
    info "Detached ${MOUNT_DIR} only after -force"
    return 0
  fi
  info "Could not detach ${MOUNT_DIR}; leaving it for the runner to reap"
  return 1
}

cleanup() {
  # Whatever the script was already exiting with. Teardown reports problems but
  # does not get to change the verdict — see the rm below.
  local rc=$?

  if [[ "$MOUNTED" == "1" ]]; then
    detach_mount || true
    MOUNTED=0
  fi

  local candidate
  for candidate in "${APP_CANDIDATE_PATHS[@]}"; do
    if [[ -d "$candidate" ]]; then
      run_as_root rm -rf "$candidate" || true
    fi
  done

  local index
  for ((index = 0; index < ${#APP_RESTORE_PATHS[@]}; index++)); do
    local restore_path="${APP_RESTORE_PATHS[$index]}"
    local backup_path="${APP_BACKUP_PATHS[$index]}"
    if [[ -d "$backup_path" ]]; then
      run_as_root mkdir -p "$(dirname "$restore_path")" || true
      run_as_root mv "$backup_path" "$restore_path" || true
    fi
  done

  # This `rm` used to be the last statement in an EXIT trap, so its status
  # became the script's status. On 2026-08-21 the rc.9 release printed
  # "macOS installer smoke completed" and then failed the step anyway: the DMG
  # was still mounted, every path under it was read-only, and `rm` exited 1.
  # `create-release` needs this job, so a teardown race blocked publication of
  # a build that had passed every check.
  if ! rm -rf "$TMP_DIR" 2>/dev/null; then
    info "Could not fully remove ${TMP_DIR}; leaving it for the runner to reap"
  fi

  return "$rc"
}
trap cleanup EXIT
trap 'cleanup; exit 130' INT
trap 'cleanup; exit 143' TERM

run_backup_restore_dry_run_proof() {
  info "Running backup/restore dry-run proof"
  backup_existing_apps

  local candidate
  for candidate in "${APP_CANDIDATE_PATHS[@]}"; do
    mkdir -p "$candidate/Contents/MacOS"
    printf 'smoke-created\n' > "$candidate/Contents/MacOS/smoke-created-marker"
  done

  cleanup
  trap - EXIT

  local restore_path
  for restore_path in "${APP_RESTORE_PATHS[@]}"; do
    [[ -d "$restore_path" ]] || fatal "Dry-run restore failed for $restore_path"
    [[ ! -f "$restore_path/Contents/MacOS/smoke-created-marker" ]] \
      || fatal "Dry-run cleanup leaked smoke artifact at $restore_path"
  done
  info "Backup/restore dry-run proof completed"
}

if [[ "$DRY_RUN_BACKUP_RESTORE_PROOF" == "1" ]]; then
  run_backup_restore_dry_run_proof
  exit 0
fi

[[ -d "$ASSETS_DIR" ]] || fatal "Asset directory not found: $ASSETS_DIR"

DMG_PATH="$(resolve_asset "$DMG_NAME")"
PKG_PATH="$(resolve_asset "$PKG_NAME")"

info "Using DMG: $DMG_PATH"
info "Using PKG: $PKG_PATH"

backup_existing_apps

mkdir -p "$MOUNT_DIR"
info "Mounting DMG"
hdiutil attach "$DMG_PATH" -nobrowse -readonly -mountpoint "$MOUNT_DIR" -quiet
MOUNTED=1

DMG_APP_PATH="$MOUNT_DIR/$APP_NAME"
DMG_BINARY_PATH="$DMG_APP_PATH/Contents/MacOS/maekon"
DMG_SIDECAR_PATH="$DMG_APP_PATH/Contents/MacOS/maekon-sandbox-worker"
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
APP_BINARY_PATH="$APP_INSTALL_PATH/Contents/MacOS/maekon"
APP_SIDECAR_PATH="$APP_INSTALL_PATH/Contents/MacOS/maekon-sandbox-worker"
[[ -x "$APP_BINARY_PATH" ]] || fatal "Installed app binary missing: $APP_BINARY_PATH"
[[ -x "$APP_SIDECAR_PATH" ]] || fatal "Installed sandbox worker sidecar missing: $APP_SIDECAR_PATH"
info "Detected installed app at $APP_INSTALL_PATH"

info "Validating binary from PKG installation"
validate_binary_format "$APP_BINARY_PATH"
validate_binary_format "$APP_SIDECAR_PATH"
run_gui_bootstrap_smoke "$APP_BINARY_PATH" "pkg"

info "macOS installer smoke completed"
