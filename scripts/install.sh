#!/usr/bin/env bash

set -euo pipefail

REPO="${ONESHIM_REPOSITORY:-pseudotop/oneshim-client}"
VERSION="${ONESHIM_VERSION:-latest}"
INSTALL_DIR="${ONESHIM_INSTALL_DIR:-$HOME/.local/bin}"
BASE_URL="${ONESHIM_RELEASE_BASE_URL:-}"
REQUIRE_SIGNATURE="${ONESHIM_REQUIRE_SIGNATURE:-0}"
UPDATE_SIGNATURE_PUBLIC_KEY="${ONESHIM_UPDATE_PUBLIC_KEY:-GIdf7Wg4kvvvoT7jR0xwKLKna8hUR1kvowONbHbPz1E=}"
BINARY_NAME="oneshim"
SIDECAR_NAME="oneshim-sandbox-worker"

usage() {
  cat <<'EOF'
Maekon release installer (macOS/Linux)

Usage:
  ./scripts/install.sh [options]

Options:
  --version <tag>          Release tag (e.g. v0.0.4). Default: latest
  --install-dir <path>     Installation directory. Default: ~/.local/bin
  --repo <owner/name>      GitHub repository. Default: pseudotop/oneshim-client
  --base-url <url>         Release asset base URL override (for local/rehearsal mirrors)
  --require-signature      Fail if Ed25519 signature verification cannot be completed
  -h, --help               Show help

Environment:
  ONESHIM_VERSION
  ONESHIM_INSTALL_DIR
  ONESHIM_REPOSITORY
  ONESHIM_RELEASE_BASE_URL
  ONESHIM_REQUIRE_SIGNATURE=1
  ONESHIM_UPDATE_PUBLIC_KEY=<base64 ed25519 public key>
EOF
}

info() {
  printf '[INFO] %s\n' "$*"
}

warn() {
  printf '[WARN] %s\n' "$*" >&2
}

fatal() {
  printf '[ERROR] %s\n' "$*" >&2
  exit 1
}

print_completion_banner() {
  local banner_path=""
  local script_dir=""

  if [[ -n "${BASH_SOURCE[0]:-}" ]]; then
    script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
  fi

  if [[ -n "$script_dir" && -f "$script_dir/../assets/brand/cli-banner.txt" ]]; then
    banner_path="$script_dir/../assets/brand/cli-banner.txt"
  elif [[ -f "assets/brand/cli-banner.txt" ]]; then
    banner_path="assets/brand/cli-banner.txt"
  fi

  printf '\n'
  if [[ -n "$banner_path" ]]; then
    cat "$banner_path"
  else
    cat <<'EOF'
Maekon
local context agent
EOF
  fi
  printf '\n\n'
}

download_file() {
  local url="$1"
  local output="$2"

  if command -v curl >/dev/null 2>&1; then
    curl --fail --silent --show-error --location "$url" --output "$output"
    return 0
  fi

  if command -v wget >/dev/null 2>&1; then
    wget -qO "$output" "$url"
    return 0
  fi

  fatal "Neither curl nor wget is installed."
}

sha256_file() {
  local file="$1"

  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$file" | awk '{print tolower($1)}'
    return 0
  fi

  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$file" | awk '{print tolower($1)}'
    return 0
  fi

  if command -v openssl >/dev/null 2>&1; then
    openssl dgst -sha256 "$file" | awk '{print tolower($NF)}'
    return 0
  fi

  fatal "No SHA-256 tool found (sha256sum/shasum/openssl)."
}

install_executable() {
  local source_path="$1"
  local target_path="$2"

  if install -m 0755 "$source_path" "$target_path" 2>/dev/null; then
    :
  else
    cp "$source_path" "$target_path"
    chmod 0755 "$target_path"
  fi
}

install_sidecar_if_present() {
  local target_dir="$1"

  if [[ -z "${SOURCE_SIDECAR:-}" ]]; then
    warn "Sandbox worker sidecar was not found in the archive; sandboxed automation actions may be unavailable."
    return 0
  fi

  install_executable "$SOURCE_SIDECAR" "$target_dir/$SIDECAR_NAME"
  info "Installed sandbox worker: $target_dir/$SIDECAR_NAME"
}

verify_signature_with_python() {
  local payload_path="$1"
  local sig_path="$2"
  local public_key_b64="$3"
  local python_cmd="$4"

  "$python_cmd" - "$payload_path" "$sig_path" "$public_key_b64" <<'PY'
import base64
import sys
from pathlib import Path

payload_path = Path(sys.argv[1])
sig_path = Path(sys.argv[2])
pubkey_b64 = sys.argv[3].split()[0]

try:
    from nacl.exceptions import BadSignatureError
    from nacl.signing import VerifyKey
except Exception as exc:
    print(f"PyNaCl is required for signature verification: {exc}", file=sys.stderr)
    sys.exit(10)

sig_tokens = sig_path.read_text(encoding="utf-8").split()
if not sig_tokens:
    print("Signature file is empty", file=sys.stderr)
    sys.exit(11)

signature = base64.b64decode(sig_tokens[0])
public_key = base64.b64decode(pubkey_b64)

if len(public_key) != 32:
    print(f"Invalid public key length: {len(public_key)} (expected 32)", file=sys.stderr)
    sys.exit(12)

if len(signature) != 64:
    print(f"Invalid signature length: {len(signature)} (expected 64)", file=sys.stderr)
    sys.exit(13)

payload = payload_path.read_bytes()

try:
    VerifyKey(public_key).verify(payload, signature)
except BadSignatureError:
    print("Signature verification failed", file=sys.stderr)
    sys.exit(14)
PY
}

normalize_tag() {
  local version="$1"
  if [[ "$version" == "latest" ]]; then
    printf 'latest'
  else
    version="${version#v}"
    printf 'v%s' "$version"
  fi
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
          fatal "Unsupported Linux architecture: $arch_name (supported: x86_64)"
          ;;
      esac
      ;;
    *)
      fatal "Unsupported OS: $os_name (supported: macOS, Linux)"
      ;;
  esac
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --version)
      [[ $# -ge 2 ]] || fatal "--version requires a value"
      VERSION="$2"
      shift 2
      ;;
    --install-dir)
      [[ $# -ge 2 ]] || fatal "--install-dir requires a value"
      INSTALL_DIR="$2"
      shift 2
      ;;
    --repo)
      [[ $# -ge 2 ]] || fatal "--repo requires a value"
      REPO="$2"
      shift 2
      ;;
    --base-url)
      [[ $# -ge 2 ]] || fatal "--base-url requires a value"
      BASE_URL="$2"
      shift 2
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
      fatal "Unknown option: $1 (use --help)"
      ;;
  esac
done

OS_NAME="$(uname -s)"
ARCH_NAME="$(uname -m)"

# Linux runtime dependency check
if [[ "$OS_NAME" == "Linux" ]]; then
  MISSING=""
  for lib in libwebkit2gtk-4.1-0 libgtk-3-0 libappindicator3-1; do
    if ! dpkg -s "$lib" >/dev/null 2>&1; then
      MISSING="$MISSING $lib"
    fi
  done
  if [[ -n "$MISSING" ]]; then
    warn "Missing required system packages:$MISSING"
    warn "Install with: sudo apt install$MISSING"
    # Interactive prompt only if stdin is a terminal
    if [[ -t 0 ]]; then
      echo ""
      read -p "Install now? [y/N] " -n 1 -r
      echo
      if [[ $REPLY =~ ^[Yy]$ ]]; then
        sudo apt install -y $MISSING
      fi
    fi
  fi
fi

ASSET_NAME="$(detect_asset_name "$OS_NAME" "$ARCH_NAME")"
TAG_NAME="$(normalize_tag "$VERSION")"

if [[ "$TAG_NAME" == "latest" ]]; then
  DEFAULT_BASE_URL="https://github.com/$REPO/releases/latest/download"
else
  DEFAULT_BASE_URL="https://github.com/$REPO/releases/download/$TAG_NAME"
fi

if [[ -z "${BASE_URL}" ]]; then
  BASE_URL="$DEFAULT_BASE_URL"
fi
BASE_URL="${BASE_URL%/}"

ARTIFACT_URL="$BASE_URL/$ASSET_NAME"
CHECKSUM_URL="$ARTIFACT_URL.sha256"
SIGNATURE_URL="$ARTIFACT_URL.sig"

TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/oneshim-install.XXXXXX")"
trap 'rm -rf "$TMP_DIR"' EXIT

ARTIFACT_PATH="$TMP_DIR/$ASSET_NAME"
CHECKSUM_PATH="$ARTIFACT_PATH.sha256"
SIGNATURE_PATH="$ARTIFACT_PATH.sig"
EXTRACT_DIR="$TMP_DIR/extract"

info "Repository: $REPO"
info "Version: $TAG_NAME"
info "Asset: $ASSET_NAME"
info "Install dir: $INSTALL_DIR"
info "Base URL: $BASE_URL"

info "Downloading artifact"
download_file "$ARTIFACT_URL" "$ARTIFACT_PATH"

info "Downloading checksum"
download_file "$CHECKSUM_URL" "$CHECKSUM_PATH"

info "Verifying SHA-256 checksum"
EXPECTED_SHA="$(awk 'NF { print tolower($1); exit }' "$CHECKSUM_PATH")"
if [[ -z "$EXPECTED_SHA" ]]; then
  fatal "Checksum file is empty: $CHECKSUM_URL"
fi
ACTUAL_SHA="$(sha256_file "$ARTIFACT_PATH")"
if [[ "$EXPECTED_SHA" != "$ACTUAL_SHA" ]]; then
  fatal "Checksum mismatch. expected=$EXPECTED_SHA actual=$ACTUAL_SHA"
fi
info "Checksum verification passed"

SIGNATURE_DOWNLOADED=0
if download_file "$SIGNATURE_URL" "$SIGNATURE_PATH"; then
  SIGNATURE_DOWNLOADED=1
else
  if [[ "$REQUIRE_SIGNATURE" == "1" ]]; then
    fatal "Failed to download signature file while --require-signature is enabled: $SIGNATURE_URL"
  fi
  warn "Signature file download failed. Continuing because --require-signature is not enabled."
fi

if [[ "$SIGNATURE_DOWNLOADED" == "1" ]]; then
  PYTHON_CMD=""
  if command -v python3 >/dev/null 2>&1; then
    PYTHON_CMD="python3"
  elif command -v python >/dev/null 2>&1; then
    PYTHON_CMD="python"
  fi

  if [[ -n "$PYTHON_CMD" ]]; then
    if verify_signature_with_python "$ARTIFACT_PATH" "$SIGNATURE_PATH" "$UPDATE_SIGNATURE_PUBLIC_KEY" "$PYTHON_CMD"; then
      info "Ed25519 signature verification passed"
    else
      if [[ "$REQUIRE_SIGNATURE" == "1" ]]; then
        fatal "Signature verification failed or PyNaCl is missing."
      fi
      warn "Signature verification skipped (PyNaCl missing or verification failed)."
      warn "Run with --require-signature to fail closed."
    fi
  else
    if [[ "$REQUIRE_SIGNATURE" == "1" ]]; then
      fatal "Python is required for signature verification."
    fi
    warn "Python is not available, skipping signature verification."
  fi
fi

mkdir -p "$EXTRACT_DIR"
tar -xzf "$ARTIFACT_PATH" -C "$EXTRACT_DIR"

SOURCE_BINARY="$EXTRACT_DIR/$BINARY_NAME"
if [[ ! -f "$SOURCE_BINARY" ]]; then
  SOURCE_BINARY="$(find "$EXTRACT_DIR" -maxdepth 3 -type f -name "$BINARY_NAME" | head -n 1)"
fi
if [[ -z "${SOURCE_BINARY:-}" || ! -f "$SOURCE_BINARY" ]]; then
  fatal "Could not locate '$BINARY_NAME' inside archive."
fi

SOURCE_SIDECAR="$EXTRACT_DIR/$SIDECAR_NAME"
if [[ ! -f "$SOURCE_SIDECAR" ]]; then
  SOURCE_SIDECAR="$(find "$EXTRACT_DIR" -maxdepth 3 -type f -name "$SIDECAR_NAME" | head -n 1)"
fi
if [[ -z "${SOURCE_SIDECAR:-}" || ! -f "$SOURCE_SIDECAR" ]]; then
  SOURCE_SIDECAR=""
fi

mkdir -p "$INSTALL_DIR"
TARGET_BINARY="$INSTALL_DIR/$BINARY_NAME"

# macOS: create .app bundle for proper dock icon rendering (glassmorphism, shadow, squircle)
if [[ "$OS_NAME" == "Darwin" ]]; then
  ICON_SOURCE="$EXTRACT_DIR/icon.icns"
  APP_DIR="${ONESHIM_APP_DIR:-$HOME/Applications}"
  APP_BUNDLE="$APP_DIR/Maekon.app"

  if [[ -f "$ICON_SOURCE" ]]; then
    info "Creating macOS .app bundle: $APP_BUNDLE"
    mkdir -p "$APP_BUNDLE/Contents/MacOS" "$APP_BUNDLE/Contents/Resources"

    install_executable "$SOURCE_BINARY" "$APP_BUNDLE/Contents/MacOS/$BINARY_NAME"
    install_sidecar_if_present "$APP_BUNDLE/Contents/MacOS"

    cp "$ICON_SOURCE" "$APP_BUNDLE/Contents/Resources/icon.icns"

    # Extract version from tag for Info.plist
    APP_VERSION="${TAG_NAME#v}"
    if [[ "$APP_VERSION" == "latest" || -z "$APP_VERSION" ]]; then
      APP_VERSION="0.0.0"
    fi

    cat > "$APP_BUNDLE/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key>
  <string>Maekon</string>
  <key>CFBundleDisplayName</key>
  <string>Maekon</string>
  <key>CFBundleIdentifier</key>
  <string>com.oneshim.client</string>
  <key>CFBundleVersion</key>
  <string>$APP_VERSION</string>
  <key>CFBundleShortVersionString</key>
  <string>$APP_VERSION</string>
  <key>CFBundleExecutable</key>
  <string>oneshim</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>LSMinimumSystemVersion</key>
  <string>10.15</string>
  <key>CFBundleIconFile</key>
  <string>icon.icns</string>
  <key>NSAppTransportSecurity</key>
  <dict>
    <key>NSExceptionDomains</key>
    <dict>
      <key>127.0.0.1</key>
      <dict>
        <key>NSExceptionAllowsInsecureHTTPLoads</key>
        <true/>
        <key>NSIncludesSubdomains</key>
        <false/>
      </dict>
      <key>localhost</key>
      <dict>
        <key>NSExceptionAllowsInsecureHTTPLoads</key>
        <true/>
        <key>NSIncludesSubdomains</key>
        <false/>
      </dict>
    </dict>
  </dict>
</dict>
</plist>
PLIST

    # Symlink binary to INSTALL_DIR for CLI access
    mkdir -p "$INSTALL_DIR"
    ln -sf "$APP_BUNDLE/Contents/MacOS/$BINARY_NAME" "$TARGET_BINARY"
    info "Installed: $APP_BUNDLE"
    info "Symlinked: $TARGET_BINARY → $APP_BUNDLE/Contents/MacOS/$BINARY_NAME"
    info "Launch: open $APP_BUNDLE"
  else
    warn "icon.icns not found in archive; installing as bare binary (no .app bundle)"
    install_executable "$SOURCE_BINARY" "$TARGET_BINARY"
    install_sidecar_if_present "$INSTALL_DIR"
    info "Installed: $TARGET_BINARY"
  fi
else
  # Linux: install bare binary
  install_executable "$SOURCE_BINARY" "$TARGET_BINARY"
  install_sidecar_if_present "$INSTALL_DIR"
  info "Installed: $TARGET_BINARY"
fi

if [[ ":$PATH:" != *":$INSTALL_DIR:"* ]]; then
  warn "$INSTALL_DIR is not in PATH."
  warn "Add this line to your shell profile:"
  warn "  export PATH=\"$INSTALL_DIR:\$PATH\""
fi

print_completion_banner
info "Run command: $BINARY_NAME"
