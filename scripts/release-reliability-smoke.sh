#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="${ONESHIM_SMOKE_REPO_ROOT:-$(cd "$SCRIPT_DIR/.." && pwd)}"
CARGO_CMD="$ROOT_DIR/scripts/cargo-cache.sh"

ASSETS_DIR="${ONESHIM_SMOKE_ASSETS_DIR:-dist}"
INSTALL_SCRIPT="${ONESHIM_INSTALL_SCRIPT:-$ROOT_DIR/scripts/install.sh}"
HOST="${ONESHIM_SMOKE_HOST:-127.0.0.1}"
PORT="${ONESHIM_SMOKE_PORT:-18090}"
RUN_UPDATER_TESTS="${ONESHIM_SMOKE_RUN_UPDATER_TESTS:-1}"
REQUIRE_SIGNATURE="${ONESHIM_SMOKE_REQUIRE_SIGNATURE:-0}"
ASSET_NAME="${ONESHIM_SMOKE_ASSET_NAME:-}"
INSTALL_DIR="${ONESHIM_SMOKE_INSTALL_DIR:-}"

# Path that tauri::generate_context!() validates at compile time (from tauri.conf.json).
FRONTEND_DIST="$ROOT_DIR/crates/oneshim-web/frontend/dist"
FRONTEND_DIST_STUB=0

info() {
  printf '[SMOKE] %s\n' "$*"
}

fatal() {
  printf '[SMOKE][ERROR] %s\n' "$*" >&2
  exit 1
}

usage() {
  cat <<'EOF'
Release reliability smoke test (macOS/Linux)

Usage:
  ./scripts/release-reliability-smoke.sh [options]

Options:
  --assets-dir <path>        Asset directory containing archive + .sha256
  --install-script <path>    Installer script path (default: scripts/install.sh)
  --asset-name <name>        Override archive file name (auto-detected by OS/arch if omitted)
  --install-dir <path>       Installer target directory (temp dir by default)
  --host <host>              Local HTTP host for serving artifacts
  --port <port>              Local HTTP port for serving artifacts
  --skip-updater-tests       Skip updater release reliability tests
  --require-signature        Require Ed25519 signature verification in the installer
  -h, --help                 Show help
EOF
}

detect_asset_name() {
  local os_name="$1"
  local arch_name="$2"

  case "$os_name" in
    Darwin)
      printf 'oneshim-macos-universal.tar.gz'
      ;;
    Linux)
      case "$arch_name" in
        x86_64 | amd64)
          printf 'oneshim-linux-x64.tar.gz'
          ;;
        *)
          fatal "Unsupported Linux architecture: $arch_name"
          ;;
      esac
      ;;
    *)
      fatal "Unsupported OS: $os_name"
      ;;
  esac
}

validate_binary_format() {
  local bin_path="$1"
  local file_info
  file_info="$(file "$bin_path")"
  info "Binary format: $file_info"

  case "$(uname -s)" in
    Darwin)
      if ! echo "$file_info" | grep -qE "Mach-O|universal binary"; then
        fatal "Binary is not a valid macOS executable: $file_info"
      fi
      ;;
    Linux)
      if ! echo "$file_info" | grep -qE "ELF"; then
        fatal "Binary is not a valid Linux executable: $file_info"
      fi
      ;;
  esac
}

run_gui_bootstrap_smoke() {
  local bin_path="$1"
  local label="$2"
  local log_path="$LOG_DIR/gui-bootstrap-${label}.log"

  info "Running GUI bootstrap smoke (${label})"

  # Tauri is a GUI app — launch briefly and check for panics.
  # On headless CI, display/GTK/WebKit failures are expected and non-fatal.
  set +e
  ONESHIM_DISABLE_TRAY=1 "$bin_path" >"$log_path" 2>&1 &
  local pid=$!
  sleep 3
  if kill -0 "$pid" >/dev/null 2>&1; then
    # SIGTERM first; escalate to SIGKILL if the process doesn't exit.
    # Tauri may block on WindowServer/WKWebView init on headless macOS CI
    # and never respond to SIGTERM, causing `wait` to hang forever.
    kill "$pid" >/dev/null 2>&1 || true
    local grace=0
    while kill -0 "$pid" >/dev/null 2>&1 && [[ "$grace" -lt 5 ]]; do
      sleep 1
      grace=$((grace + 1))
    done
    if kill -0 "$pid" >/dev/null 2>&1; then
      info "SIGTERM did not terminate process $pid after 5s — sending SIGKILL"
      kill -9 "$pid" >/dev/null 2>&1 || true
    fi
  fi
  wait "$pid" 2>/dev/null
  local rc=$?
  set -e

  # Fatal: Rust panics or tokio runtime double-init
  if grep -qE "Cannot start a runtime from within a runtime|Cannot drop a runtime|panicked at|SIGABRT|stack backtrace" "$log_path"; then
    # Exclude known headless-CI-expected messages that may contain "panic" substring
    if ! grep -qE "Failed to initialize gtk|no display|cannot open display|WKWebView" "$log_path" || \
       grep -qE "Cannot start a runtime|Cannot drop a runtime|SIGABRT|stack backtrace" "$log_path"; then
      cat "$log_path"
      fatal "GUI bootstrap smoke detected runtime/panic failure (${label})"
    fi
  fi

  # On headless CI, many non-zero exits are expected (no display server).
  # Only flag truly unexpected crashes (segfault=139, abort=134).
  if [[ "$rc" -eq 139 || "$rc" -eq 134 ]]; then
    cat "$log_path"
    fatal "GUI bootstrap smoke crashed (rc=$rc, ${label})"
  fi

  info "GUI bootstrap smoke OK (rc=$rc, ${label})"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --assets-dir)
      [[ $# -ge 2 ]] || fatal "--assets-dir requires a value"
      ASSETS_DIR="$2"
      shift 2
      ;;
    --install-script)
      [[ $# -ge 2 ]] || fatal "--install-script requires a value"
      INSTALL_SCRIPT="$2"
      shift 2
      ;;
    --asset-name)
      [[ $# -ge 2 ]] || fatal "--asset-name requires a value"
      ASSET_NAME="$2"
      shift 2
      ;;
    --install-dir)
      [[ $# -ge 2 ]] || fatal "--install-dir requires a value"
      INSTALL_DIR="$2"
      shift 2
      ;;
    --host)
      [[ $# -ge 2 ]] || fatal "--host requires a value"
      HOST="$2"
      shift 2
      ;;
    --port)
      [[ $# -ge 2 ]] || fatal "--port requires a value"
      PORT="$2"
      shift 2
      ;;
    --skip-updater-tests)
      RUN_UPDATER_TESTS=0
      shift
      ;;
    --require-signature)
      REQUIRE_SIGNATURE=1
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

[[ -d "$ASSETS_DIR" ]] || fatal "Asset directory not found: $ASSETS_DIR"
[[ -f "$INSTALL_SCRIPT" ]] || fatal "Installer script not found: $INSTALL_SCRIPT"

if [[ -z "$ASSET_NAME" ]]; then
  ASSET_NAME="$(detect_asset_name "$(uname -s)" "$(uname -m)")"
fi

ARTIFACT_PATH="$ASSETS_DIR/$ASSET_NAME"
CHECKSUM_PATH="$ARTIFACT_PATH.sha256"
SIGNATURE_PATH="$ARTIFACT_PATH.sig"
[[ -f "$ARTIFACT_PATH" ]] || fatal "Artifact missing: $ARTIFACT_PATH"
[[ -f "$CHECKSUM_PATH" ]] || fatal "Checksum missing: $CHECKSUM_PATH"
if [[ "$REQUIRE_SIGNATURE" == "1" ]]; then
  [[ -f "$SIGNATURE_PATH" ]] || fatal "Signature missing while --require-signature is enabled: $SIGNATURE_PATH"
fi

TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/oneshim-release-smoke.XXXXXX")"
SERVER_LOG="$TMP_DIR/http.log"
LOG_DIR="${ONESHIM_SMOKE_LOG_DIR:-$TMP_DIR}"
if [[ -z "$INSTALL_DIR" ]]; then
  INSTALL_DIR="$TMP_DIR/bin"
fi
mkdir -p "$LOG_DIR"

cleanup() {
  if [[ -n "${SERVER_PID:-}" ]] && kill -0 "$SERVER_PID" >/dev/null 2>&1; then
    kill "$SERVER_PID" >/dev/null 2>&1 || true
    wait "$SERVER_PID" >/dev/null 2>&1 || true
  fi
  if [[ "$FRONTEND_DIST_STUB" == "1" ]]; then
    rm -rf "$FRONTEND_DIST"
  fi
  rm -rf "$TMP_DIR"
}
trap cleanup EXIT

PYTHON_CMD=""
if command -v python3 >/dev/null 2>&1; then
  PYTHON_CMD="python3"
elif command -v python >/dev/null 2>&1; then
  PYTHON_CMD="python"
else
  fatal "Python is required to host local release assets"
fi

wait_for_port() {
  local host="$1" port="$2" max_wait="$3"
  local elapsed=0
  while [ "$elapsed" -lt "$max_wait" ]; do
    if (echo >/dev/tcp/"$host"/"$port") 2>/dev/null; then
      return 0
    fi
    sleep 0.5
    elapsed=$((elapsed + 1))
  done
  return 1
}

info "Serving assets from $ASSETS_DIR on http://$HOST:$PORT"
(
  cd "$ASSETS_DIR"
  "$PYTHON_CMD" -m http.server "$PORT" --bind "$HOST" >"$SERVER_LOG" 2>&1
) &
SERVER_PID=$!
if ! wait_for_port "$HOST" "$PORT" 10; then
  kill -0 "$SERVER_PID" >/dev/null 2>&1 || true
  fatal "HTTP server not listening on $HOST:$PORT within 5 seconds"
fi
kill -0 "$SERVER_PID" >/dev/null 2>&1 || fatal "Failed to start local HTTP server"

BASE_URL="http://$HOST:$PORT"
info "Running installer against local base URL"
INSTALL_ARGS=(--install-dir "$INSTALL_DIR" --base-url "$BASE_URL")
if [[ "$REQUIRE_SIGNATURE" == "1" ]]; then
  INSTALL_ARGS+=(--require-signature)
fi
bash "$INSTALL_SCRIPT" "${INSTALL_ARGS[@]}"

TARGET_BIN="$INSTALL_DIR/oneshim"
[[ -x "$TARGET_BIN" ]] || fatal "Installed binary not found: $TARGET_BIN"

TARGET_SIDECAR="$INSTALL_DIR/oneshim-sandbox-worker"
if [[ "$(uname -s)" == "Darwin" ]]; then
  APP_BUNDLE="${ONESHIM_APP_DIR:-$HOME/Applications}/Maekon.app"
  APP_SIDECAR="$APP_BUNDLE/Contents/MacOS/oneshim-sandbox-worker"
  [[ -x "$APP_SIDECAR" ]] || fatal "Installed sandbox worker sidecar not found: $APP_SIDECAR"
else
  [[ -x "$TARGET_SIDECAR" ]] || fatal "Installed sandbox worker sidecar not found: $TARGET_SIDECAR"
fi

info "Validating binary format"
validate_binary_format "$TARGET_BIN"

if [[ "$(uname -s)" == "Darwin" ]]; then
  run_gui_bootstrap_smoke "$TARGET_BIN" "installed-binary"
fi

if [[ "$RUN_UPDATER_TESTS" == "1" ]]; then
  info "Running updater reliability regression tests"
  # tauri::generate_context!() validates frontendDist at compile time.
  # Create a minimal stub when running outside a full frontend build (e.g. CI smoke).
  if [[ ! -d "$FRONTEND_DIST" ]]; then
    info "Creating frontendDist stub for tauri::generate_context!() compilation"
    mkdir -p "$FRONTEND_DIST"
    printf '<!doctype html><html><body></body></html>\n' > "$FRONTEND_DIST/index.html"
    FRONTEND_DIST_STUB=1
  fi
  "$CARGO_CMD" test --manifest-path "$ROOT_DIR/Cargo.toml" -p oneshim-app release_reliability_ -- --nocapture
fi

info "Release reliability smoke completed"
