# macOS Info.plist Version Skew After Updater Install

**Spec**: Phase 4 Updater Hardening holistic review — Important Issue I-3
**Scope**: macOS `.app` bundle version metadata lifecycle across in-place updater installs.

---

## Executive summary

The Phase 4 updater installs new versions by replacing the **executable binary** at `current_exe()`, which on macOS resolves to
`/Applications/OneShim.app/Contents/MacOS/oneshim` — a file **inside** the `.app` bundle, not the bundle itself.

After a successful update:

| Surface | Source of version string | After update |
|---|---|---|
| Running app (gRPC, UI, telemetry) | binary — compiled-in `CARGO_PKG_VERSION` | **N+1** (new) |
| Finder → Get Info | `.app/Contents/Info.plist::CFBundleShortVersionString` | **N** (stale) |
| Spotlight, Launchpad, mdfind | `Info.plist` indexed metadata | **N** (stale) |
| `system_profiler SPApplicationsDataType` | `Info.plist` | **N** (stale) |
| Code signature | signed over bundle contents including `Info.plist` + binary | **Invalid** (binary hash differs from original signature) |

**Consequence**: users see inconsistent version numbers depending on which tool they check; macOS Gatekeeper may refuse to launch the bundle after the next reboot if the quarantine bit is set or if hardened-runtime integrity checks fail.

This document exists because the Phase 4 implementation **ships with this skew**. The hardened fix (bundle-level replacement + re-sign) is tracked as a follow-up; this guide is the interim operational playbook.

---

## Why bundle-level replacement was deferred from Phase 4

Phase 4's scope was D9 multi-key trust, D10 rollout defense, D11 auto-rollback. The binary-only install path predated Phase 4 and remained unchanged to keep the merge non-breaking (see `v0.4.40-rc.1` target).

Bundle-level replacement requires:

1. **Updater payload format change** — archive the whole `.app` rather than just the binary. Increases download size (resources + Info.plist + Frameworks/ + `_CodeSignature/`) from ~30 MB to ~60–80 MB.
2. **Re-sign step** — bundle-level replacement changes the directory inode on disk; the existing Developer ID signature on the user's installed `.app` becomes invalid. Either:
   - Ship the update as a *signed* bundle and overwrite atomically via `rename()` or `ditto --rsrc`.
   - Apply the new signature in-place after install using `codesign --force --deep --sign <Developer ID>` — requires the signing identity to ship with the installer, which is **not acceptable** (private key exposure).
3. **Elevated permissions** — `/Applications/*.app` is writable only by the installing user. If the app was dragged to `/Applications` by root (admin installer), `.app/Contents/` may require `sudo` to rewrite. The current binary-only path squeaks past this because the binary itself inherits the user's write perms after the original drag.

Phase 4 opted to ship the working binary-only path and document the skew rather than block on bundle replacement design.

---

## Diagnosis

### Checking for skew on a user's machine

```bash
# Running app's compiled-in version
/Applications/OneShim.app/Contents/MacOS/oneshim --version

# Info.plist metadata (what Finder shows)
plutil -p /Applications/OneShim.app/Contents/Info.plist \
  | grep -E 'CFBundle(Version|ShortVersionString)'

# Combined diagnostic one-liner
printf "binary: %s\nplist:  %s\n" \
  "$(/Applications/OneShim.app/Contents/MacOS/oneshim --version 2>&1 | head -1)" \
  "$(plutil -extract CFBundleShortVersionString raw /Applications/OneShim.app/Contents/Info.plist)"
```

### Verifying code signature integrity

```bash
codesign --verify --verbose=4 /Applications/OneShim.app 2>&1
```

After an updater install, expect output like:
```
a sealed resource is missing or invalid
```
or
```
main executable failed strict validation
```

This is the expected state after in-place binary replacement and does **not** indicate tampering.

### Checking Gatekeeper acceptance

```bash
spctl --assess --verbose=4 /Applications/OneShim.app 2>&1
```

Typical post-update output:
```
/Applications/OneShim.app: rejected
source=no usable signature
```

Gatekeeper assessment failure does **not** immediately block launch on a machine that already ran the app (Gatekeeper caches the first-launch approval), but it **will** block launch on a reboot after the `com.apple.quarantine` attribute is re-added (e.g. via mobile-device management reprovisioning).

---

## Recovery

### For end users

If Finder shows a stale version or the app refuses to launch after reboot:

1. Download the latest stable installer DMG from the GitHub releases page.
2. Drag the new `OneShim.app` from the DMG to `/Applications`, replacing the existing one.
3. Launch once from Finder; macOS Gatekeeper will re-register the signature.

This is the same recovery flow as a first-time install.

### For operators (fleet-wide rollout)

Distribute a post-update healing script via MDM:

```bash
#!/bin/bash
# heal-oneshim-bundle.sh — re-sync Info.plist and re-register signature
set -euo pipefail

APP=/Applications/OneShim.app
BINARY_VER=$("$APP/Contents/MacOS/oneshim" --version | awk '{print $NF}')
PLIST_VER=$(plutil -extract CFBundleShortVersionString raw "$APP/Contents/Info.plist")

if [ "$BINARY_VER" = "$PLIST_VER" ]; then
  echo "No skew: $BINARY_VER"
  exit 0
fi

echo "Skew detected: binary=$BINARY_VER plist=$PLIST_VER"
echo "Rewriting Info.plist CFBundleShortVersionString → $BINARY_VER"

plutil -replace CFBundleShortVersionString -string "$BINARY_VER" "$APP/Contents/Info.plist"
plutil -replace CFBundleVersion -string "$BINARY_VER" "$APP/Contents/Info.plist"

# Re-run Launch Services so Spotlight/Finder pick up the new plist.
/System/Library/Frameworks/CoreServices.framework/Versions/A/Frameworks/LaunchServices.framework/Versions/A/Support/lsregister \
  -f "$APP"

# Note: this does NOT restore code signature integrity. An untrusted-machine
# launch after reboot will still fail Gatekeeper. Ship this alongside a
# full-bundle reinstall plan if signature integrity matters.
echo "Info.plist synced. Signature remains invalid — reinstall from DMG to restore."
```

The script syncs `Info.plist` but does not re-sign; Gatekeeper will still reject the bundle on cold launch. For complete healing, a full-bundle reinstall is required.

---

## Long-term fix (post-Phase 4)

Tracking ticket: *(TBD — file as separate PR when scheduled)*

Recommended approach:

1. **Installer format**: ship updates as a full `.app` tar+zstd bundle. Keep the binary-only path as a fast-fallback for patch releases that only touch the binary.
2. **Atomic swap**: download the new bundle to `~/Library/Application Support/OneShim/pending/OneShim.app.new`, then `ditto --rsrc` the entire tree into place while the current process holds its own binary lock. On Unix this works because files are identified by inode; the running process keeps executing the old binary's inode until next launch.
3. **Re-sign on server**: the CI release workflow signs the assembled bundle with the Developer ID certificate before upload. The installer client never touches the signing identity.
4. **Gate on `codesign --verify` post-install**: abort and roll back if the newly installed bundle fails signature check.

Implementation estimate: ~1 week for one engineer, plus a macOS CI runner with signing-identity access.

---

## References

- Internal Phase 4 updater hardening design — D9/D10/D11 scope definition
- [Windows Rollback Spike](updater-rollback-windows.md) — parallel platform issue for reference
- [Apple — Bundle Versioning](https://developer.apple.com/library/archive/documentation/CoreFoundation/Conceptual/CFBundles/BundleTypes/BundleTypes.html#//apple_ref/doc/uid/10000123i-CH101-SW1) — `CFBundleVersion` / `CFBundleShortVersionString` semantics
- [Apple — Code Signing Troubleshooting](https://developer.apple.com/library/archive/technotes/tn2318/_index.html) — diagnosing `codesign` failures
