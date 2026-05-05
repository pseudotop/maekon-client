English | [한국어](./autostart.ko.md)

# Autostart Operations Guide

## Overview

Maekon supports launching automatically when the user logs in.

- **Policy**: opt-in — users must explicitly enable it
- **How to enable**: in-app Settings → Startup toggle
- **Supported platforms**: macOS, Windows, Linux (desktop session required)

When autostart is enabled, Maekon starts in the background after login and immediately begins collecting work context.

---

## Per-platform behavior

### macOS

| Item | Value |
|------|-------|
| Registration path | `~/Library/LaunchAgents/com.maekon.app.plist` |
| Mechanism | `launchctl load` / `launchctl unload` |
| Single-instance | Unix domain socket (tauri-plugin-single-instance) |

**Note**: Only works correctly with Gatekeeper-notarized binaries. Unsigned builds may fail to register autostart.

### Windows

| Item | Value |
|------|-------|
| Registration path | Registry `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` |
| Mechanism | Windows Registry API |
| Single-instance | Named pipe (tauri-plugin-single-instance) |

**Note**: Standard user accounts can write under `HKCU`, so administrator privileges are not required.

### Linux

| Item | Value |
|------|-------|
| Primary mechanism | systemd user service `~/.config/systemd/user/maekon.service` (`Type=notify`) |
| Secondary fallback | XDG Autostart `~/.config/autostart/maekon.desktop` |
| Single-instance | D-Bus name `com.maekon.app.SingleInstance` |

On Linux, Maekon first checks whether `systemctl --user` is available and prefers the systemd user service path. In environments without systemd (some containers, older distros), it automatically falls back to the XDG Autostart `.desktop` file.

**Meaning of `Type=notify`**: Maekon signals "ready" to systemd via `sd_notify(3)`. This lets systemd track whether Maekon has actually finished initializing. If no signal arrives within `TimeoutStartSec=30`, the service is treated as failed to start.

---

## Linux environment support matrix

| Environment | Supported | Notes |
|-------------|-----------|-------|
| systemd user session (most desktop distros) | ✅ | `Type=notify` provides accurate readiness signal |
| Without systemd (XDG fallback) | ✅ | `.desktop` file is used; no readiness signal |
| Snap package | ❌ | Use Snap's built-in autostart instead |
| Flatpak package | ❌ | Use the Flatpak background portal API instead |
| Headless environments (SSH, no display) | ❌ | Desktop session required |

> **Minimum systemd version**: systemd 219+ required (Ubuntu 20.04+, Fedora 33+, Debian 10+ all qualify).
> On systemd 218 or earlier, the XDG fallback is applied automatically.

**How support is detected**: at startup, Maekon probes the environment in this order.

1. `FLATPAK_ID` environment variable present → Flatpak detected, autostart disabled
2. Snap detected (`SNAP` environment variable) → Snap detected, autostart disabled
3. Both `$DISPLAY` / `$WAYLAND_DISPLAY` and `$DBUS_SESSION_BUS_ADDRESS` missing → headless, disabled
4. Result of `systemctl --user is-system-running` decides between systemd and XDG branches

---

## Migration (PR-B1 → PR-B2 upgrade)

PR-B1 (pre-v0.4.40) shipped a `Type=simple` systemd unit. PR-B2 (Maekon v0.4.41+) switches to `Type=notify` for a more accurate readiness signal.

**Two PR-B1 variants**: the PR-B1 era has two known unit-file shapes.

- v0.4.40-rc.1 / rc.2: `Description=MAEKON Desktop Agent`
- v0.4.40-rc.3 / v0.4.40: `Description=Maekon Desktop Agent`

PR-B2's automatic migration recognises both variants (both hashes are registered in `KNOWN_PRIOR_HASHES`).

### Automatic migration

On the first run of Maekon v0.4.41+, the following happens automatically:

1. Compute the SHA-256 hash of `~/.config/systemd/user/maekon.service`
2. Compare it against the known PR-B1 template hashes
3. **On match**: overwrite the file with the PR-B2 template
   - `daemon-reload` is deferred to the next login (the currently running service is not interrupted)
   - Logs `autostart: service file migrated`
4. **On mismatch** (user customisation detected):
   - Skip automatic migration
   - Logs `WARN autostart: service file has local modifications, skipping auto-migration`
   - Provides manual migration instructions (see below)

### Manual migration (for users who customised the service file)

```bash
# 1. Back up the existing customisation
cp ~/.config/systemd/user/maekon.service \
   ~/.config/systemd/user/maekon.service.backup

# 2. Apply the required changes:
#    - Type=simple  →  Type=notify
#    - Add: NotifyAccess=main
#    - Add: TimeoutStartSec=30
#    - Keep any existing Environment= and similar customisations

# 3. Verify the changes
grep -E "^Type=|^NotifyAccess=|^TimeoutStartSec=" \
  ~/.config/systemd/user/maekon.service

# 4. Reload systemd and restart the service
systemctl --user daemon-reload
systemctl --user restart maekon.service

# 5. Check the status
systemctl --user status maekon.service
```

**Example service file after the migration (key sections)**:

```ini
[Service]
Type=notify
NotifyAccess=main
TimeoutStartSec=30
ExecStart=/usr/local/bin/maekon
Restart=on-failure
RestartSec=5
```

---

## Troubleshooting

### "The Settings → Startup toggle is greyed out"

The toggle is disabled in environments that do not support autostart.

1. Hover the toggle to see the tooltip message.
   > Tooltip text is defined in `crates/maekon-web/frontend/src/i18n/locales/en.json` (`settings.autostart.unsupported_*`).
2. Per-environment guidance:
   - **Snap users**: configure autostart with `snap services` or via the "Run on system startup" option in Snap Center.
   - **Flatpak users**: autostart through the Flatpak background portal API is configured in GNOME Settings (or KDE System Settings) under "Run in background". The `~/.var/app/...` directory only stores user data and is unrelated to autostart configuration.
   - **Headless users**: SSH sessions are not autostart targets. Configure it from a desktop session.

### "I enabled it but Maekon doesn't start after login"

**Step 1: check the systemd service status**

```bash
systemctl --user status maekon.service
journalctl --user -u maekon.service -n 50
```

**Step 2: common causes and fixes**

| Symptom | Cause | Fix |
|---------|-------|-----|
| `timeout: starting` | Exceeds `TimeoutStartSec=30` (HDDs, large DB) | Raise to `TimeoutStartSec=60` |
| `Failed to connect to bus` | D-Bus not running | `systemctl --user start dbus` or log in again |
| `No such file or directory` | Binary path changed | Run `which maekon`, then update the `ExecStart=` line in the service file |
| Exits immediately, no logs | Duplicate-instance detection | Inspect existing processes: `pgrep -a maekon` |

**How to adjust `TimeoutStartSec`**:

```bash
# Edit ~/.config/systemd/user/maekon.service
sed -i 's/^TimeoutStartSec=.*/TimeoutStartSec=60/' \
  ~/.config/systemd/user/maekon.service
systemctl --user daemon-reload
systemctl --user restart maekon.service
```

**Step 3: log locations**

- **macOS**: `~/Library/Logs/maekon/`
- **Windows**: `%LOCALAPPDATA%\maekon\logs\`
- **Linux**: `~/.local/share/maekon/logs/` or `journalctl --user -u maekon`

### "Migration was skipped because the service file is customised"

If you see the following log entry:

```
WARN autostart: service file has local modifications, skipping auto-migration
```

Follow the [Manual migration](#manual-migration-for-users-who-customised-the-service-file) procedure above. Keep your existing customisation (`Environment=`, `ConditionEnvironment=`, etc.) and only add `Type=notify`, `NotifyAccess=main`, and `TimeoutStartSec=30`.

### "Two Maekon processes appear after every login"

Single-instance detection is not working.

**Diagnostics**:

```bash
# List running maekon processes
pgrep -a maekon

# Check whether the D-Bus name is owned
dbus-send --session --print-reply \
  --dest=org.freedesktop.DBus \
  /org/freedesktop/DBus \
  org.freedesktop.DBus.ListNames \
  | grep maekon
```

**Possible causes**:

- **Running from a headless SSH session**: with `$DBUS_SESSION_BUS_ADDRESS` unset, D-Bus connection fails and single-instance detection is disabled (duplicate processes can result).
- **Two autostart paths registered**: a systemd service and an XDG `.desktop` file are both registered.

**Manual cleanup**:

```bash
# Remove the XDG .desktop file (when redundant under a systemd setup)
rm -f ~/.config/autostart/maekon.desktop

# Stop the duplicate processes
pkill -f maekon
# Then restart cleanly with a single process
systemctl --user start maekon.service
```

---

## Single-instance behavior

Maekon allows only one running instance at a time.

| Situation | Behavior |
|-----------|----------|
| First launch | Starts normally |
| Second launch attempt | Focuses the first instance's window, then exits immediately |

Per-platform signal mechanism:

- **macOS**: Unix domain socket (tauri-plugin-single-instance)
- **Windows**: Named pipe
- **Linux**: D-Bus name `com.maekon.app.SingleInstance`

**Known limitation — Wayland tray-only startup**:

When the first instance starts in tray-only mode without ever showing the main window (some Wayland environments), a second launch (e.g., a dock icon click) may send the focus signal but no window appears.

This was accepted as a known limitation in PR-B1 risk register §13. If it happens:

1. Click the tray icon → choose "Show window"
2. Or run `maekon --show-window` from a terminal (a `window.create()` fallback is planned for a follow-up PR).

---

## References

- **PR-B1 spec**: `docs/superpowers/specs/2026-04-25-phase9-pr-b-autostart-ipc-foundation-design.md`
- **PR-B2 spec**: `docs/superpowers/specs/2026-04-25-phase9-pr-b2-autostart-linux-deep-design.md`
- **ADR-019**: wire-code infrastructure (`crates/maekon-core/src/error_codes/autostart.rs`)
- **Single-instance plugin**: tauri-plugin-single-instance v2
- **Install guide**: [`docs/install.md`](./install.md) — binary installation (also covers legacy filename / environment-variable names)
