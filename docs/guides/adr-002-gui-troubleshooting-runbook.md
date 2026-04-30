# ADR-002 GUI Troubleshooting Runbook

Operator reference for diagnosing GUI V2 permission and runtime failures on macOS, Windows, and Linux.

## Prerequisites

| Requirement | Detail |
|-------------|--------|
| HMAC secret | `ONESHIM_GUI_TICKET_HMAC_SECRET` environment variable must be set (non-empty) |
| OS permissions | Accessibility/AT-SPI permission granted for the Maekon process |
| GUI feature | Enabled in `AppConfig` (automation section) |
| Overlay driver | Platform overlay adapter registered (Tauri WebView or native window) |

## macOS Permission Flow

1. Open **System Preferences > Privacy & Security > Accessibility**.
2. Add and enable the Maekon application.
3. If the permission prompt was previously dismissed, reset and re-grant:

```bash
tccutil reset Accessibility com.oneshim.app
```

4. Restart the Maekon process after granting.

> **Note:** Screen Recording permission is also required for screen capture (`Privacy & Security > Screen Recording`).

## Windows Permission Flow

UIA (UI Automation) works without elevation by default for most applications.

**For protected apps (elevated/admin processes):**
- Run ONESHIM as Administrator, or
- Use *Accessibility Insights for Windows* to verify UIA tree visibility.

**Diagnostics:**
- Open **Event Viewer > Windows Logs > Application** and filter for UIA-related errors.
- Verify that `UIAutomationCore.dll` loads correctly (check `sxstrace` if COM activation fails).

## Linux Permission Flow

1. Verify AT-SPI bus is running:

```bash
busctl --user introspect org.a11y.Bus /org/a11y/bus
```

2. If AT-SPI is not active, enable it:

```bash
gsettings set org.gnome.desktop.interface toolkit-accessibility true
```

3. Verify `DBUS_SESSION_BUS_ADDRESS` is set in the Maekon process environment.

4. For Wayland sessions, ensure XWayland compatibility or that the application uses the AT-SPI D-Bus interface directly.

## Common Failure Signatures

| Symptom | HTTP | Cause | Action |
|---------|------|-------|--------|
| `503 Service Unavailable` on session create | 503 | `ONESHIM_GUI_TICKET_HMAC_SECRET` env var missing or empty | Set the env var and restart |
| `503 Service Unavailable` on session create | 503 | GUI feature disabled in config | Enable `automation.gui_enabled` in config |
| `403 Forbidden` on scene analysis | 403 | Accessibility permission denied by OS | Grant OS-level accessibility permission (see platform sections above) |
| `409 Conflict` on confirm/execute | 409 | Focused window changed between propose and execute | Retry the flow from the `POST /sessions` step |
| `422 Unprocessable` on execute | 422 | Execution ticket expired (default TTL: 30 seconds) | Re-confirm the candidate to obtain a fresh ticket |
| `422 Unprocessable` on execute | 422 | Ticket nonce already consumed (replay) | Create a new session; each ticket nonce is single-use |
| Empty candidate list (0 elements) | 200 | Both OCR and Accessibility extraction returned nothing | Check screen capture + accessibility permissions; lower `min_confidence` |

## Diagnostic Commands

**Log verbosity:**

```bash
RUST_LOG=oneshim_automation=debug cargo run -p oneshim-app
```

**Per-platform checks:**

| OS | Command | Purpose |
|----|---------|---------|
| macOS | `tccutil reset Accessibility com.oneshim.app` | Reset accessibility permission to re-prompt |
| macOS | `sqlite3 ~/Library/Application\ Support/com.apple.TCC/TCC.db "SELECT * FROM access WHERE service='kTCCServiceAccessibility'"` | Query current TCC grants |
| Linux | `busctl --user introspect org.a11y.Bus /org/a11y/bus` | Verify AT-SPI D-Bus service |
| Linux | `gsettings get org.gnome.desktop.interface toolkit-accessibility` | Check accessibility toggle |
| Windows | Event Viewer > Windows Logs > Application (filter: UIA) | Check UIA errors |

## Session Lifecycle Debugging

1. **Create** (`POST /api/automation/gui/sessions`) -- if this fails with 503, check HMAC secret and feature flag.
2. **Highlight** (`POST .../highlight`) -- if no candidates appear, capture permissions are the likely issue.
3. **Confirm** (`POST .../confirm`) -- 409 means focus drifted; the user switched windows.
4. **Execute** (`POST .../execute`) -- 422 means the ticket is stale or replayed.

Check `AuditLogger` entries (via `GET /api/automation/audit/recent`) for a timeline of state transitions and denied operations.

## Related Documents

- [GUI Interaction Contract](../contracts/gui-interaction-contract.md) -- schema definitions and error mapping
- [GUI V2 API Examples](../contracts/gui-interaction-v2-examples.md) -- cURL request/response examples
- [GUI Security Review](../security/adr-002-gui-security-review.md) -- threat model and mitigations
