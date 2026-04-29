#!/usr/bin/env bash
# Maekon macOS 제거 스크립트
# launchctl unload + plist 삭제 + 바이너리 삭제

set -euo pipefail

BINARY_NAME="oneshim"
INSTALL_DIR="/usr/local/bin"
PLIST_LABEL="com.oneshim.agent"
PLIST_PATH="$HOME/Library/LaunchAgents/$PLIST_LABEL.plist"

# 색상 출력
info()  { echo -e "\033[1;34m[INFO]\033[0m  $*"; }
ok()    { echo -e "\033[1;32m[OK]\033[0m    $*"; }

# LaunchAgent 해제
if [[ -f "$PLIST_PATH" ]]; then
    info "LaunchAgent 해제"
    launchctl unload "$PLIST_PATH" 2>/dev/null || true
    rm -f "$PLIST_PATH"
    ok "plist 삭제 완료"
else
    info "plist 파일 없음 (이미 제거됨)"
fi

# 심볼릭 링크 및 바이너리 삭제
if [[ -f "$INSTALL_DIR/$BINARY_NAME" || -L "$INSTALL_DIR/$BINARY_NAME" ]]; then
    info "바이너리/심볼릭 링크 삭제: $INSTALL_DIR/$BINARY_NAME"
    sudo rm -f "$INSTALL_DIR/$BINARY_NAME"
    ok "바이너리 삭제 완료"
else
    info "바이너리 없음 (이미 제거됨)"
fi

# .app 번들 삭제
for APP_BUNDLE in "/Applications/Maekon.app" "/Applications/ONESHIM.app"; do
    if [[ -d "$APP_BUNDLE" ]]; then
        info ".app 번들 삭제: $APP_BUNDLE"
        sudo rm -rf "$APP_BUNDLE"
        ok ".app 번들 삭제 완료"
    fi
done

ok "Maekon 제거 완료!"
