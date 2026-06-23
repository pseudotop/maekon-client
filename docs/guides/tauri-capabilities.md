# Tauri Capability Permissions

How MAEKON uses Tauri v2 capabilities to scope IPC permissions per window.

## Security Model

Tauri v2 uses a **capability-based security model**. Each window is assigned
one or more capability files that whitelist exactly which app IPC commands,
plugin commands, and core APIs the window's JavaScript context may invoke. Any
call not listed in the window's capability set is blocked by the Tauri runtime.

Capability files live in `src-tauri/capabilities/` and are referenced by the
runtime automatically (Tauri discovers all `.json` files in that directory).

## Window Inventory

| Window label | Source URL | Purpose | Capability file | App IPC commands |
|--------------|------------|---------|-----------------|------------------|
| `main` | `tauri.conf.json` main window, production app assets or dev `http://127.0.0.1:5273` | Primary dashboard | `default.json` | Explicit `allow-*` inventory generated from `src-tauri/build.rs` |
| `magic-overlay` | `WebviewUrl::App("overlay.html")` | Transparent always-on-top overlay for coaching, detection, suggestions | `overlay.json` | Overlay-only subset |
| `tracking-panel` | `WebviewUrl::App("tracking-panel.html")` | Compact floating panel showing tracking status | `tracking-panel.json` | Tracking-panel subset |
| `tracking-border-*` | `WebviewUrl::App("overlay.html")` | Passive recording-border surfaces | No matching capability | None |

## Remote Content Inventory

The Tauri window inventory currently contains no `WebviewUrl::External(...)`
windows and the frontend does not define iframes. Runtime network egress exists
for providers, update checks, and loopback dashboard/API calls, but those paths
are not WebView origins and do not receive Tauri IPC authority.

If a future feature needs a remote URL, OAuth page, documentation pane, or
embedded iframe, it must use a dedicated window or browser handoff that has no
app-command capability unless a separate security review documents the exact
commands and origin boundary.

## Capability Details

### `default.json` (main window)

```
Identifier: default
Windows:    ["main"]
```

**Permissions granted:**

| Permission | Purpose |
|------------|---------|
| `core:default` | Basic Tauri runtime APIs |
| `core:window:default` | Standard window queries (size, position, etc.) |
| `core:window:allow-hide` | Hide the main window to system tray |
| `core:window:allow-show` | Restore from tray |
| `core:window:allow-minimize` | Minimize |
| `core:window:allow-maximize` | Maximize |
| `core:window:allow-unmaximize` | Restore from maximized |
| `core:window:allow-is-maximized` | Query maximize state |
| `core:window:allow-set-focus` | Bring to front |
| `core:window:allow-start-dragging` | Custom title bar drag |
| `core:window:allow-start-resize-dragging` | Custom resize handle |
| `core:window:allow-close` | Close the window |
| `core:event:allow-listen` | Subscribe to Tauri events |
| `core:event:allow-unlisten` | Unsubscribe from events |
| `notification:default` | Desktop notification API |
| `global-shortcut:allow-register` | Register global keyboard shortcuts |
| `global-shortcut:allow-unregister` | Unregister shortcuts |
| `allow-*` app commands | Main-window-only command inventory from `src-tauri/build.rs` |

This is the most permissive capability -- the main window is the primary
user interface and needs full window management, event handling, and
notification access. It is also the only window allowed to invoke MAEKON app
commands such as settings, automation, analysis, OAuth, updater preview, local
auth token retrieval, capture, audio, tray, consent, and debug commands.

### `overlay.json` (magic-overlay window)

```
Identifier: overlay
Windows:    ["magic-overlay"]
```

**Permissions granted:**

| Permission | Purpose |
|------------|---------|
| `core:default` | Basic runtime APIs |
| `core:event:allow-listen` | Receive overlay state events from Rust |
| `core:event:allow-unlisten` | Clean up listeners |
| `core:window:allow-set-ignore-cursor-events` | Pass mouse clicks through the transparent overlay |
| `core:window:allow-show` | Show the overlay |
| `core:window:allow-hide` | Hide the overlay |
| `notification:default` | Desktop notifications for coaching nudges |
| Overlay `allow-*` app commands | Suggestions panel, coaching feedback, detection overlay, automation confirmation, Codex approval |

The overlay is intentionally restricted. It cannot resize, drag, close, or
maximize itself. The `set-ignore-cursor-events` permission is critical --
it allows the overlay to toggle between interactive (showing UI elements)
and pass-through (invisible to mouse) modes. Its app commands are limited to
the commands that `overlay.html` invokes directly:

- `toggle_suggestions_panel`, `get_pending_suggestions`, suggestion feedback/history/stats/replay commands
- `toggle_automation_confirm`, `confirm_automation_command`, `respond_codex_approval`
- `refresh_detection_overlay`, `toggle_detection_overlay`
- `get_capture_status`, `dismiss_coaching_message`, `submit_coaching_feedback`

### `tracking-panel.json` (tracking-panel window)

```
Identifier: tracking-panel
Windows:    ["tracking-panel"]
```

**Permissions granted:**

| Permission | Purpose |
|------------|---------|
| `core:default` | Basic runtime APIs |
| `core:event:allow-listen` | Receive tracking state events |
| `core:event:allow-unlisten` | Clean up listeners |
| `core:event:allow-emit` | Emit events back to Rust (e.g., user actions) |
| `core:window:allow-show` | Show the panel |
| `core:window:allow-hide` | Hide the panel |
| `core:window:allow-set-size` | Resize the panel dynamically |
| `core:window:allow-start-dragging` | Allow the user to drag the panel |
| `core:window:allow-set-position` | Programmatic position control |
| Tracking-panel `allow-*` app commands | Capture status/actions, focus toggle, suggestions open, main-window open, tray quit |

The tracking panel has `allow-emit` (which the overlay does not) because it
needs to send user interaction events back to the Rust backend. It also has
position/size control for its floating-window UX. Its app commands are limited
to the commands that `tracking-panel.html` invokes directly:

- `get_capture_status`, `get_connection_status`, `get_panel_position`, `save_panel_position`
- `trigger_manual_capture`, `analyze_current_scene`, `toggle_capture_pause`, `set_indicator_visible`
- `get_focus_mode_status`, `toggle_focus_mode`, `toggle_suggestions_panel`
- `show_main_window`, `simulate_tray_action`

## Adding a New IPC Command

When you add a new `#[tauri::command]` in `src-tauri/src/commands/`:

### Step 1: Implement the Command

```rust
// src-tauri/src/commands/my_feature.rs
#[tauri::command]
pub async fn my_new_command(state: State<'_, RuntimeState>) -> Result<String, String> {
    // ...
}
```

### Step 2: Register in the Tauri Builder

In `src-tauri/src/main.rs`, add the command to `.invoke_handler(tauri::generate_handler![...])`.

### Step 3: Register in the Build Manifest

In `src-tauri/build.rs`, add the command name to `APP_COMMANDS`. This lets
`tauri_build::AppManifest::commands(...)` generate the app-command permission
identifiers used by the capability files. The identifiers are kebab-case
(`get_app_build_info` -> `allow-get-app-build-info` / `deny-get-app-build-info`).

### Step 4: Scope the Command to the Intended Window

Add the generated `allow-<command>` permission only to the capability for the
window that needs it. Today that is normally `default.json` (`main`) only. Do
not add app-command permissions to `overlay.json` or `tracking-panel.json`
unless the command is explicitly reviewed for those surfaces.

If your command uses a Tauri plugin API (e.g., `notification`, `dialog`,
`global-shortcut`), also add the corresponding plugin permission to the
relevant capability file.

For new plugin permissions, edit the appropriate `.json` file in
`src-tauri/capabilities/`:

```json
{
  "permissions": [
    "existing:permission",
    "new-plugin:allow-operation"
  ]
}
```

Only add the permission to windows that genuinely need it. Follow the
principle of least privilege.

## Adding a New Window

1. Define the window in `src-tauri/src/main.rs` (or create it dynamically
   via `WebviewWindowBuilder`).
2. Create a new capability file: `src-tauri/capabilities/<window-name>.json`.
3. Set `"windows": ["<window-label>"]` and list only the permissions the
   window needs.
4. Start with `core:default` + `core:event:allow-listen` +
   `core:event:allow-unlisten` as the minimal set.
5. Add app-command permissions only after deciding whether the new surface is
   trusted to invoke each command.

## Principle of Least Privilege

Each window should have the minimum permissions required:

- **Overlay**: No close, no resize, no emit -- it is controlled by Rust-side
  events and only needs to toggle cursor pass-through plus its reviewed overlay
  app-command subset.
- **Tracking panel**: No close, no maximize -- but needs emit + position
  control for its floating UX plus its reviewed tracking app-command subset.
- **Main window**: Full window management -- it is the primary interface.

When in doubt, omit the permission and add it only when a runtime error
confirms it is needed.

## Related Files

- `src-tauri/capabilities/default.json`
- `src-tauri/capabilities/overlay.json`
- `src-tauri/capabilities/tracking-panel.json`
- `src-tauri/tauri.conf.json` -- CSP, window definitions, bundle config
- `src-tauri/build.rs` -- App command manifest used for generated ACLs
- `src-tauri/src/commands/` -- IPC command implementations
- `src-tauri/src/main.rs` -- Command registration and window creation
- `src-tauri/tests/ipc_command_contract.rs` -- Static guard for command/capability drift
