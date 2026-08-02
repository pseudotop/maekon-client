#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
cd "$repo_root"

cargo_bin="${CARGO:-cargo}"
target="${TARGET:-x86_64-unknown-linux-gnu}"
lock_file="${CARGO_LOCK:-Cargo.lock}"
production_tree_file="${PRODUCTION_TREE_FILE:-}"
reverse_tree_file="${WEBDRIVER_REVERSE_TREE_FILE:-}"
release_features="${RELEASE_FEATURES:-grpc,systemd-notify,linux-sandbox,lan-sync,stt,download}"

fail() {
  echo "webdriver security isolation failure: $*" >&2
  exit 1
}

if [[ -n "$production_tree_file" ]]; then
  production_tree="$(cat "$production_tree_file")"
else
  production_tree="$(
    "$cargo_bin" tree \
      --locked \
      --target "$target" \
      -p maekon-app \
      --features "$release_features"
  )"
fi

if grep -Eq '(^|[[:space:]])(webkit2gtk|webkit2gtk-sys) v' <<<"$production_tree"; then
  fail "the shipped Linux feature graph contains WebKit2GTK"
fi

if grep -Eq '(^|[[:space:]])(glib|glib-sys) v0\.18\.' <<<"$production_tree"; then
  fail "the shipped Linux feature graph contains glib 0.18.x"
fi

if grep -Eq '(^|[[:space:]])tauri-plugin-wdio-webdriver v' <<<"$production_tree"; then
  fail "the test-only embedded WebDriver plugin leaked into the shipped Linux feature graph"
fi

if ! awk '
  function flush() {
    if ((name == "glib" || name == "glib-sys") && version ~ /^0[.]18[.]/) {
      found = 1
    }
  }
  /^\[\[package\]\]/ {
    flush()
    name = ""
    version = ""
    next
  }
  /^name = "/ {
    name = $0
    sub(/^name = "/, "", name)
    sub(/"$/, "", name)
    next
  }
  /^version = "/ {
    version = $0
    sub(/^version = "/, "", version)
    sub(/"$/, "", version)
    next
  }
  END {
    flush()
    exit(found ? 0 : 1)
  }
' "$lock_file"; then
  echo "webdriver security isolation passed: no glib 0.18.x lockfile entry"
  exit 0
fi

grep -Eq '^tauri-plugin-wdio-webdriver = \{ workspace = true, optional = true \}$' \
  src-tauri/Cargo.toml \
  || fail "tauri-plugin-wdio-webdriver must remain optional"
grep -Fq 'webdriver = ["dep:tauri-plugin-wdio", "dep:tauri-plugin-wdio-webdriver"]' \
  src-tauri/Cargo.toml \
  || fail "the embedded driver must remain isolated behind the webdriver feature"

if [[ -n "$reverse_tree_file" ]]; then
  reverse_tree="$(cat "$reverse_tree_file")"
else
  reverse_tree="$(
    "$cargo_bin" tree \
      --all-features \
      --target "$target" \
      --locked \
      -p maekon-app \
      -i glib@0.18.5 \
      --prefix depth \
      --no-dedupe
  )"
fi

grep -Eq '^0glib v0\.18\.5$' <<<"$reverse_tree" \
  || fail "the reviewed glib 0.18.5 reverse graph is unavailable or changed"
grep -Eq '^[0-9]+tauri-plugin-wdio-webdriver v1\.2\.0' <<<"$reverse_tree" \
  || fail "glib 0.18.5 is not rooted through the reviewed tauri-plugin-wdio-webdriver 1.2.0"

if ! awk '
  function clear_deeper(depth, cursor) {
    for (cursor = depth + 1; cursor < 64; cursor++) {
      delete path[cursor]
    }
  }
  /^[0-9]+/ {
    match($0, /^[0-9]+/)
    depth = substr($0, RSTART, RLENGTH) + 0
    node = substr($0, RLENGTH + 1)
    path[depth] = node
    clear_deeper(depth)

    if (node ~ /^maekon-app v/) {
      isolated = 0
      for (cursor = 0; cursor <= depth; cursor++) {
        if (path[cursor] ~ /^tauri-plugin-wdio-webdriver v1[.]2[.]0/) {
          isolated = 1
        }
      }
      if (!isolated) {
        exit 1
      }
      app_paths++
    }
  }
  END {
    if (app_paths == 0) {
      exit 1
    }
  }
' <<<"$reverse_tree"; then
  fail "at least one glib 0.18.5 path reaches maekon-app outside the reviewed test-only plugin"
fi

echo "webdriver security isolation passed: glib 0.18.5 is test-only and absent from the shipped Linux feature graph"
