# Tauri Capability Permissions

How ONESHIM uses Tauri v2 capabilities to scope IPC permissions per window.

## Security Model

Tauri v2 uses a **capability-based security model**. Each window is assigned
one or more capability files that whitelist exactly which IPC commands and
core APIs the window's JavaScript context may invoke. Any call not listed in
the window's capability set is silently blocked by the Tauri runtime.

Capability files live in `src-tauri/capabilities/` and are referenced by the
runtime automatically (Tauri discovers all `.json` files in that directory).

## Window Inventory

| Window label | Purpose | Capability file |
|--------------|---------|-----------------|
| `main` | Primary dashboard (web UI served by Axum or Tauri dev server) | `default.json` |
| `magic-overlay` | Transparent always-on-top overlay for coaching, detection, suggestions | `overlay.json` |
| `tracking-panel` | Compact floating panel showing tracking status | `tracking-panel.json` |

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

This is the most permissive capability -- the main window is the primary
user interface and needs full window management, event handling, and
notification access.

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

The overlay is intentionally restricted. It cannot resize, drag, close, or
maximize itself. The `set-ignore-cursor-events` permission is critical --
it allows the overlay to toggle between interactive (showing UI elements)
and pass-through (invisible to mouse) modes.

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

The tracking panel has `allow-emit` (which the overlay does not) because it
needs to send user interaction events back to the Rust backend. It also has
position/size control for its floating-window UX.

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

### Step 3: No Capability Change Needed for Custom Commands

Tauri v2 custom commands (those defined with `#[tauri::command]`) are
allowed by default in all windows that have `core:default`. The capability
allowlist applies only to **built-in Tauri APIs** (window management,
events, notifications, file system, etc.).

If your command uses a Tauri plugin API (e.g., `notification`, `dialog`,
`global-shortcut`), you must add the corresponding plugin permission to the
relevant capability file.

### Step 4: If Adding a New Plugin Permission

Edit the appropriate `.json` file in `src-tauri/capabilities/`:

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

## Principle of Least Privilege

Each window should have the minimum permissions required:

- **Overlay**: No close, no resize, no emit -- it is controlled entirely by
  Rust-side events and only needs to toggle cursor pass-through.
- **Tracking panel**: No close, no maximize -- but needs emit + position
  control for its floating UX.
- **Main window**: Full window management -- it is the primary interface.

When in doubt, omit the permission and add it only when a runtime error
confirms it is needed.

## Related Files

- `src-tauri/capabilities/default.json`
- `src-tauri/capabilities/overlay.json`
- `src-tauri/capabilities/tracking-panel.json`
- `src-tauri/tauri.conf.json` -- CSP, window definitions, bundle config
- `src-tauri/src/commands/` -- IPC command implementations (14 modules)
- `src-tauri/src/main.rs` -- Command registration and window creation
