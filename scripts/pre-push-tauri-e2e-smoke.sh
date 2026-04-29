#!/usr/bin/env bash
set -euo pipefail

timeout_secs="${ONESHIM_PRE_PUSH_STDIN_TIMEOUT_SECS:-1}"

while IFS=' ' read -r -t "$timeout_secs" local_ref local_sha remote_ref remote_sha; do
  case "$remote_ref" in
    refs/tags/v*)
      if [ "$(uname)" != "Darwin" ]; then
        echo "⏭️  Tauri E2E smoke: skipped (macOS only)"
        exit 0
      fi
      BINARY="target/debug/oneshim"
      if [ ! -f "$BINARY" ]; then
        echo "⏭️  Tauri E2E smoke: skipped (binary not built with webdriver feature)"
        exit 0
      fi
      FRONTEND_DIR="crates/oneshim-web/frontend"
      if [ ! -d "$FRONTEND_DIR/node_modules/@wdio" ]; then
        echo "⏭️  Tauri E2E smoke: skipped (@wdio not installed)"
        exit 0
      fi
      echo "🧪 Running Tauri E2E smoke test..."
      cd "$FRONTEND_DIR" && npx wdio run e2e-tauri/wdio.conf.ts --reporter=spec
      ;;
  esac
done
