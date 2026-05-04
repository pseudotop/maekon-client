# ADR-005: Tauri v2 Governance

**Date**: 2026-03-08
**Status**: Accepted
**Deciders**: Maekon maintainers
**Related**: [ADR-004: Tauri v2 Migration](ADR-004-tauri-v2-migration.md) | [ADR-006: IPC Command Contract](ADR-006-ipc-command-contract.md)

---

## Context

Maekon requires a cross-platform desktop shell for the productivity agent. The shell must:

1. Render the existing React web dashboard without a second UI codebase.
2. Provide native system integration: system tray, desktop notifications, auto-start.
3. Produce small, auditable binaries suitable for enterprise distribution.
4. Support macOS notarization and Windows code signing.
5. Expose a safe, auditable IPC surface between the Rust backend and the JavaScript frontend.

The previous GUI was implemented with the `iced` immediate-mode GUI library (crate `oneshim-ui`). This was replaced in ADR-004.

---

## Decision

Use **Tauri v2** as the desktop shell.

- **UI renderer**: WKWebView (macOS), WebView2 (Windows), WebKitGTK (Linux).
- **Backend**: Rust (`src-tauri/`), sharing the existing Cargo workspace.
- **IPC**: `tauri::command` macro for strongly-typed Rust ↔ JavaScript calls.
- **System tray**: `tauri::tray` API.
- **Notifications**: `tauri-plugin-notification`.
- **Auto-update**: `tauri-plugin-updater` with semver gating and signature verification.

---

## Alternatives Considered

| Alternative | Reason Rejected |
|-------------|----------------|
| **Keep iced** | Immediate-mode rendering degrades with complex data visualisations (timelines, heatmaps). Maintaining two UIs (iced + React) doubles frontend cost. |
| **Electron** | ~60 MB binary vs ~5 MB Tauri binary. Electron ships its own Chromium, increasing the attack surface and violating enterprise binary size budgets. |
| **Raw winit + wgpu** | No WebView integration; would require reimplementing the existing React dashboard. |
| **egui** | Same limitations as iced — immediate-mode, no WebView, limited data visualisation support. |

---

## Consequences

### Positive

- Single UI codebase (React). The existing `crates/oneshim-web/frontend/` is reused without modification.
- Cross-platform rendering consistency via the system WebView (WKWebView / WebView2).
- Binary size approximately 5 MB (Rust binary) + WebView (system-provided). No bundled Chromium.
- macOS notarization supported via standard Apple Developer ID workflow.
- Windows code signing supported via standard Authenticode workflow.
- Tauri's CSP enforcement provides a strong default for IPC security (see ADR-006).

### Negative / Risks

- **WKWebView version dependency**: macOS updates can change WKWebView behaviour. Minimum macOS version is set to 10.15 in `tauri.conf.json`. Monitor Tauri release notes after macOS major updates.
- **WebView2 on Windows**: WebView2 runtime must be present. The installer bundles the WebView2 bootstrapper for machines that do not have it.
- **Tauri IPC learning curve**: Contributors must understand the `invoke_handler!` registration pattern and the allowlist model. See ADR-006.
- **WebView memory overhead**: approximately 50 MB at runtime for the WebView process, compared to near-zero for iced in simple layouts.

---

## Update Policy

### Minor Releases (x.Y.z)

Follow Tauri minor releases within 30 days of publication. Minor releases add features and fix bugs without breaking IPC contracts.

Procedure:
1. Update `tauri` version in `src-tauri/Cargo.toml`.
2. Run `cargo check --workspace` and `cargo test --workspace`.
3. Verify `tauri.conf.json` schema URL is current.
4. Run the full CI pipeline before merging.

### Patch Releases (x.y.Z)

Apply security patches within 7 days of publication. Do not defer CVE-tagged patches.

### Major Releases (X.y.z)

Evaluate Tauri major releases case-by-case. A new ADR is required before adopting a Tauri major version. Assessment criteria:

- IPC contract compatibility (see ADR-006 versioning policy).
- WebView API changes affecting existing commands.
- Impact on macOS notarization and Windows signing workflows.
- Plugin API stability (`tauri-plugin-notification`, `tauri-plugin-updater`).

---

## Security Model

Tauri v2 enforces a Content Security Policy configured in `src-tauri/tauri.conf.json`:

```json
"security": {
  "csp": "default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self' http://127.0.0.1:10090; object-src 'none'; base-uri 'self'",
  "dangerousDisableAssetCspModification": false
}
```

Key points:

- `script-src 'self'`: Only scripts bundled with the application can execute. No inline scripts, no external CDN scripts.
- `connect-src 'self' http://127.0.0.1:10090`: The frontend may only fetch from the embedded assets and the local Axum web server. No arbitrary external connections from the WebView.
- `object-src 'none'`: Plugin objects (Flash etc.) are blocked.
- `dangerousDisableAssetCspModification: false`: Tauri enforces CSP even if the frontend tries to modify it.

The IPC surface is limited to explicitly registered commands. See ADR-006 for the full command contract.

---

## Governance Responsibilities

| Responsibility | Owner |
|---------------|-------|
| Tauri minor/patch updates | Engineering lead |
| Tauri major version evaluation | Architecture review (new ADR required) |
| CSP policy changes | Security review required before merge |
| IPC surface expansion | Architecture review (ADR-006 update required) |
| macOS notarization credentials | DevOps / release engineering |
| Windows signing certificate | DevOps / release engineering |
