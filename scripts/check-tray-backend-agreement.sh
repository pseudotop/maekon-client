#!/usr/bin/env bash
# The tray cfg gates and the vendored tray-icon backend must agree (#11006).
#
# What went wrong: `patches/tray-icon-*/src/platform_impl/mod.rs` maps every
# Linux/BSD target to `disabled.rs`, whose `TrayIcon::new` always errors — while
# `tray.rs` gated the real tray path on `feature = "app-tray"`. The release build
# passes `--features ...` without `--no-default-features`, so default's
# `app-tray` stayed on, the real path was compiled in, and the published .deb
# panicked during setup. It could not start on any Linux.
#
# Two facts, one invariant: while the vendored backend is disabled for Linux,
# tray.rs must decide by TARGET, never by feature. Otherwise a build flag can
# re-enable a backend that does not exist.
#
# Derived from both files rather than asserting a remembered state, so
# re-enabling a real Linux backend later fails here and points at the gates that
# must be revisited with it.
set -euo pipefail

CLIENT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TRAY_RS="$CLIENT_ROOT/src-tauri/src/tray.rs"

[ -f "$TRAY_RS" ] || { echo "tray.rs not found at $TRAY_RS" >&2; exit 1; }

backend_map="$(find "$CLIENT_ROOT/patches" -path '*tray-icon-*/src/platform_impl/mod.rs' -print -quit)"
[ -n "$backend_map" ] || {
  echo "vendored tray-icon platform_impl/mod.rs not found — this guard is examining nothing (#11006)" >&2
  exit 1
}

# Does the vendored crate route Linux to the disabled backend?
linux_disabled=0
if awk '/target_os = "linux"/,/^mod platform;/' "$backend_map" | grep -q 'disabled.rs'; then
  linux_disabled=1
fi

feature_gates="$(grep -c 'feature = "app-tray"' "$TRAY_RS" || true)"
# Doc comments legitimately mention the feature; only cfg predicates matter.
cfg_feature_gates="$(grep -c '^#\[cfg(.*feature = "app-tray"' "$TRAY_RS" || true)"

echo "vendored backend disables Linux: $([ "$linux_disabled" -eq 1 ] && echo yes || echo no)"
echo "tray.rs cfg predicates gated on app-tray: ${cfg_feature_gates}"

if [ "$linux_disabled" -eq 1 ] && [ "${cfg_feature_gates:-0}" -gt 0 ]; then
  cat >&2 <<EOF
tray backend disagreement (#11006):

  The vendored tray-icon crate disables the Linux/BSD backend unconditionally,
  but tray.rs still gates the real tray path on \`feature = "app-tray"\`.

  A build that enables that feature on Linux — which the release build does,
  since it passes --features without --no-default-features — compiles in a path
  whose backend always errors, and the app panics during setup.

  Decide by target on Linux, not by feature. If a real Linux backend has been
  restored in the vendored crate, revisit these gates together.
EOF
  grep -n '^#\[cfg(.*feature = "app-tray"' "$TRAY_RS" >&2 || true
  exit 1
fi

echo "tray backend agreement guard passed"
