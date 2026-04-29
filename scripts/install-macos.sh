#!/usr/bin/env bash
# Maekon macOS 설치 스크립트
# 바이너리 복사 + LaunchAgent plist 설치 + launchctl load

set -euo pipefail

BINARY_NAME="oneshim"
INSTALL_DIR="/usr/local/bin"
APP_BUNDLE="/Applications/Maekon.app"
PLIST_LABEL="com.oneshim.agent"
PLIST_DIR="$HOME/Library/LaunchAgents"
PLIST_PATH="$PLIST_DIR/$PLIST_LABEL.plist"

# 색상 출력
info()  { echo -e "\033[1;34m[INFO]\033[0m  $*"; }
ok()    { echo -e "\033[1;32m[OK]\033[0m    $*"; }
error() { echo -e "\033[1;31m[ERROR]\033[0m $*" >&2; }

# 바이너리 찾기
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
BINARY_PATH="$REPO_ROOT/target/release/$BINARY_NAME"
ICON_PATH="$REPO_ROOT/src-tauri/icons/icon.icns"

if [[ ! -f "$BINARY_PATH" ]]; then
    error "릴리즈 바이너리가 없습니다: $BINARY_PATH"
    info "먼저 빌드하세요: cargo build --release -p oneshim-app"
    exit 1
fi

# .app 번들 생성 (macOS 표준 — 독 아이콘 글래스모피즘/그림자 자동 적용)
info ".app 번들 생성: $APP_BUNDLE"
sudo mkdir -p "$APP_BUNDLE/Contents/MacOS" "$APP_BUNDLE/Contents/Resources"
sudo cp "$BINARY_PATH" "$APP_BUNDLE/Contents/MacOS/$BINARY_NAME"
sudo chmod +x "$APP_BUNDLE/Contents/MacOS/$BINARY_NAME"

if [[ -f "$ICON_PATH" ]]; then
    sudo cp "$ICON_PATH" "$APP_BUNDLE/Contents/Resources/icon.icns"
    ok "아이콘 복사 완료"
else
    error "icon.icns 파일이 없습니다: $ICON_PATH"
    info "아이콘 없이 설치를 계속합니다"
fi

# 버전 추출 (Cargo.toml에서)
APP_VERSION="0.0.0"
CARGO_TOML="$REPO_ROOT/src-tauri/Cargo.toml"
if [[ -f "$CARGO_TOML" ]]; then
    APP_VERSION="$(grep '^version' "$CARGO_TOML" | head -1 | sed 's/.*"\(.*\)".*/\1/')"
fi

sudo tee "$APP_BUNDLE/Contents/Info.plist" > /dev/null <<PLIST
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
ok ".app 번들 생성 완료"

# CLI 심볼릭 링크 생성
info "CLI 심볼릭 링크: $INSTALL_DIR/$BINARY_NAME → $APP_BUNDLE/Contents/MacOS/$BINARY_NAME"
sudo ln -sf "$APP_BUNDLE/Contents/MacOS/$BINARY_NAME" "$INSTALL_DIR/$BINARY_NAME"
ok "심볼릭 링크 생성 완료"

# LaunchAgents 디렉토리 생성
mkdir -p "$PLIST_DIR"

# plist 생성 (.app 번들 내 바이너리를 직접 참조)
info "LaunchAgent plist 생성: $PLIST_PATH"
cat > "$PLIST_PATH" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>$PLIST_LABEL</string>
    <key>ProgramArguments</key>
    <array>
        <string>$APP_BUNDLE/Contents/MacOS/$BINARY_NAME</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <false/>
    <key>StandardOutPath</key>
    <string>/tmp/oneshim-client.out.log</string>
    <key>StandardErrorPath</key>
    <string>/tmp/oneshim-client.err.log</string>
</dict>
</plist>
EOF
ok "plist 생성 완료"

# launchctl load
info "LaunchAgent 등록"
launchctl load "$PLIST_PATH" 2>/dev/null || true
ok "Maekon 설치 완료!"
info "실행: open $APP_BUNDLE"
info "다음 로그인 시 자동 시작됩니다."
