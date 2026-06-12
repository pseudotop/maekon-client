[English](./ADR-020-macos-private-api-policy.md) | [한국어](./ADR-020-macos-private-api-policy.ko.md)

# ADR-020: macOS Private API Policy (macOSPrivateApi: true)

**Status**: Accepted
**Date**: 2026-05-18
**Scope**: `src-tauri/tauri.conf.json`, `src-tauri/src/magic_overlay.rs`
**Supersedes**: none
**Related**: ADR-004 (Tauri v2 Migration), ADR-005 (Tauri v2 Governance)
**Implementation**: `src-tauri/tauri.conf.json:12`, `src-tauri/src/magic_overlay.rs:124-162`

---

## Context

`tauri.conf.json` sets `"macOSPrivateApi": true` inside the `app` object. This flag
enables Tauri's macOS-specific private API surface, which includes support for:

- **Transparent windows** — `WebviewWindowBuilder::transparent(true)` fails silently
  or panics on macOS without this flag enabled.
- **Full-content-view title bar** — `titleBarStyle: "Overlay"` combined with
  `hiddenTitle: true` (both set in `tauri.conf.json` for the main window) requires
  the private API surface to extend the WebView behind the title bar traffic lights.
- **`NSVisualEffectView` vibrancy** — available to future windows; currently unused
  but reserved.

The immediate consumer is `MagicOverlayHandle::ensure_window()` in
`src-tauri/src/magic_overlay.rs`. That function creates a full-screen, always-on-top,
transparent WebView window for the coaching / detection overlay:

```
.transparent(true)
.always_on_top(true)
.decorations(false)
.shadow(false)
```

On macOS, `.transparent(true)` on a `WebviewWindowBuilder` requires the Tauri
`macos-private-api` feature to be active at compile time **and** `macOSPrivateApi: true`
in `tauri.conf.json` at runtime. Without it, Tauri 2 on macOS renders the window with a
white or black background — the overlay is non-functional.

The main window also uses `titleBarStyle: "Overlay"` and `hiddenTitle: true`, which
produces the frameless-look title bar with inset traffic lights (standard in modern macOS
apps). This too relies on the private API surface.

### Apple notarization and App Store status

| Distribution channel | Impact |
|---|---|
| Direct download (DMG/PKG signed + notarized) | **No impact.** Apple Notarization does not reject `macOSPrivateApi` apps. Maekon passes notarization with all standard entitlements (`hardened-runtime`, screen recording, accessibility). |
| Mac App Store | **App Store submission is blocked.** MAS rejects apps that use private APIs (enforced by the App Store review). This is acceptable: Maekon is not and does not plan to be distributed via the Mac App Store. Screen-recording + accessibility entitlements are themselves MAS-incompatible. |

The flag does **not** disable Hardened Runtime (`"hardenedRuntime": true` is still set)
and does not introduce new sandboxing concerns beyond what the existing entitlements
already grant.

### SOC 2 / ISMS-P audit posture

`macOSPrivateApi: true` is visible to Apple during notarization; it does not introduce
any additional data-collection or network surface. The flag is a rendering/windowing
enabler only. Auditors asking "why is private API enabled?" must be pointed to this ADR
and to `src-tauri/src/magic_overlay.rs` as the primary consumer.

## Decision

### 1. Retain `"macOSPrivateApi": true`

Keep the flag enabled. It is required for the MagicOverlay transparent window and for the
`titleBarStyle: "Overlay"` main-window UX. Disabling it would break both.

### 2. This ADR is the single authoritative explanation

Because JSON does not support comments, `tauri.conf.json` cannot carry inline rationale.
This ADR file (`docs/architecture/ADR-020-macos-private-api-policy.md`) is the
canonical reference. ADR-005 (Tauri v2 Governance) cross-links to this ADR for the
`macOSPrivateApi` entry.

### 3. Removal trigger conditions

`macOSPrivateApi` should be re-evaluated (and potentially set to `false`) when **all**
of the following are true:

1. The MagicOverlay transparent-window feature is removed or replaced by a mechanism
   that does not require transparency (e.g. a system notification UI).
2. The main window no longer uses `titleBarStyle: "Overlay"` / `hiddenTitle: true`.
3. No future window requires `NSVisualEffectView` vibrancy.

A future ADR that supersedes this one must address all three conditions.

### 4. No App Store distribution

Maekon explicitly does not target Mac App Store distribution. The incompatibility between
MAS rules and both `macOSPrivateApi` and the `com.apple.security.device.screen-capture`
entitlement is acknowledged and accepted.

## Consequences

### Positive

- MagicOverlay transparent coaching / detection overlay works correctly on macOS.
- Main window has the expected frameless look (traffic lights inset into content area).
- Apple Notarization (DMG + PKG) succeeds with the existing entitlements.

### Negative

- Maekon cannot be submitted to the Mac App Store as long as this flag is enabled
  (compounded by the screen-recording entitlement — both are individually MAS-blocking).
- Auditors and new contributors unfamiliar with Tauri may flag the setting as suspicious
  without this ADR context.

### Neutral

- Hardened Runtime remains enabled; the flag only affects window rendering, not runtime
  security posture.
- Linux and Windows builds are unaffected by this setting.

## Alternatives Considered

**A. Disable `macOSPrivateApi` and use opaque windows.**
The MagicOverlay would need to be rebuilt without transparency — a significantly degraded
UX. The main window title bar would lose the Overlay style. Rejected.

**B. Use a separate helper process for the transparent overlay (no Tauri window).**
Possible but requires a custom Swift/ObjC helper with its own build pipeline, signing
identity, and IPC. Far more complexity than the current approach. Rejected.

**C. Ship without the overlay on macOS.**
The coaching/detection overlay is a core feature. Disabling it on macOS is not
acceptable. Rejected.

## Known Follow-ups

1. **MAS eligibility audit** — If Maekon ever evaluates App Store distribution, a
   comprehensive entitlement audit is required first (screen recording, accessibility,
   and `macOSPrivateApi` are all separately MAS-blocking). Track as a separate ADR
   when/if the business decision changes.

## Related Docs

- `docs/architecture/ADR-004-tauri-v2-migration.md` — Tauri v2 migration context
- `docs/architecture/ADR-005-tauri-governance.md` — governance rules for `tauri.conf.json`
- `src-tauri/src/magic_overlay.rs` — primary consumer (transparent overlay window)
- `src-tauri/tauri.conf.json` — configuration file that sets `macOSPrivateApi: true`
- `src-tauri/assets/maekon.entitlements` — Hardened Runtime entitlements
- `docs/guides/macos-release-signing-runbook.md` — signing + notarization procedures
