# Enterprise Deployment Guide

This guide covers mass deployment of Maekon to managed fleets using MDM, Group Policy, and Linux package managers. It is intended for IT administrators and enterprise architects.

See also: [SECURITY.md](../../SECURITY.md)

---

## Table of Contents

- [Network Requirements](#network-requirements)
- [Data Residency](#data-residency)
- [macOS MDM Deployment](#macos-mdm-deployment)
- [Windows GPO Deployment](#windows-gpo-deployment)
- [Linux Deployment](#linux-deployment)
- [Per-Tenant Configuration](#per-tenant-configuration)

---

## Network Requirements

Maekon requires outbound access from endpoints to:

| Port | Protocol | Purpose | Required |
|------|----------|---------|----------|
| 443 | HTTPS/TLS | REST API + SSE suggestions | Yes |
| 50051 | gRPC/TLS | Real-time context upload (when `grpc_enabled = true`) | Conditional |
| 10090 | HTTP | Local web dashboard (loopback only) | No — localhost only |

Port 10090 binds to `127.0.0.1` only. No firewall rule is required for the dashboard.

gRPC fallback ports (50052, 50053) are attempted automatically if 50051 fails. Configure your firewall to permit the full range if you enable gRPC.

---

## Data Residency

All captured context (screen frames, window titles, activity events) is stored on-device in:

- macOS: `~/Library/Application Support/oneshim/data/`
- Windows: `%LOCALAPPDATA%\oneshim\data\`
- Linux: `~/.local/share/oneshim/`

These storage and identifier paths currently retain `oneshim` for compatibility
with existing installs and update state. Treat them as runtime identifiers, not
the public product name.

Data is only transmitted to your connected Maekon server when `telemetry.enabled` is `true` (default: `false`). PII is filtered on-device before any upload, controlled by `privacy.pii_filter_level` (default: `"Standard"`).

Disabling telemetry keeps all data on the device and prevents any outbound data transfer. This satisfies data residency requirements for regions that prohibit cross-border data flows.

---

## macOS MDM Deployment

### Supported MDM Platforms

Jamf Pro, Mosyle Business, Kandji, and any MDM that supports `.pkg` distribution and `LaunchAgent` management.

### 1. Package the Application

Build a notarized `.dmg` from the release pipeline (see [CI Transparency](ci-transparency.md)). Wrap the `.dmg` content in a flat `.pkg` using `pkgbuild`:

```bash
pkgbuild \
  --root /path/to/Maekon.app \
  --install-location /Applications \
  --identifier com.oneshim.client \
  --version 1.0.0 \
  Maekon-1.0.0.pkg
```

Sign and notarize the `.pkg` with your Apple Developer ID Installer certificate before distributing via MDM.

### 2. LaunchAgent for Auto-Start

To start Maekon at user login, deploy a `LaunchAgent` plist via MDM:

**File path on endpoint**: `~/Library/LaunchAgents/com.oneshim.client.plist`

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.oneshim.client</string>
    <key>ProgramArguments</key>
    <array>
        <string>/Applications/Maekon.app/Contents/MacOS/oneshim</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <false/>
    <key>StandardOutPath</key>
    <string>/tmp/oneshim.out.log</string>
    <key>StandardErrorPath</key>
    <string>/tmp/oneshim.err.log</string>
</dict>
</plist>
```

Load on first deployment: `launchctl load ~/Library/LaunchAgents/com.oneshim.client.plist`

### 3. Managed Preferences (Jamf/Kandji)

Deploy a pre-configured `config.json` to each endpoint via MDM file distribution. Target path:

```
~/Library/Application Support/oneshim/config.json
```

The config file is JSON. Use the per-tenant configuration fields listed in the [Per-Tenant Configuration](#per-tenant-configuration) section below.

---

## Windows GPO Deployment

### 1. Silent MSI Install

Build the Windows installer from the release pipeline. The installer is a standard `.msi`. Silent installation via GPO:

```powershell
msiexec /i Maekon-1.0.0.msi /quiet /norestart ALLUSERS=1
```

Or via Group Policy software deployment:
1. Copy `.msi` to a network share accessible to target computers.
2. In GPMC: Computer Configuration > Policies > Software Settings > Software Installation > New Package.
3. Select "Assigned" deployment.

### 2. Registry-Based Configuration

Pre-seed the configuration by deploying `config.json` via a GPO Preference (Files item):

**Source**: `\\server\share\oneshim\config.json`
**Destination**: `%APPDATA%\oneshim\config.json`
**Action**: Replace

Alternatively, use a startup script:

```powershell
$configDir = "$env:APPDATA\oneshim"
if (-not (Test-Path $configDir)) { New-Item -ItemType Directory $configDir }
Copy-Item "\\server\share\oneshim\config.json" "$configDir\config.json" -Force
```

### 3. ADMX Template

An ADMX template is not yet published. Use the registry-based config approach above. Track the ADMX template issue on GitHub for status updates.

### 4. Auto-Start via Registry

To start Maekon at user login without relying on the built-in autostart, add a Run key via GPO Preferences:

- **Key**: `HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Run`
- **Value name**: `ONESHIM`
- **Value data**: installed binary path, for example
  `"%LOCALAPPDATA%\Maekon\oneshim.exe"`

---

## Linux Deployment

Official public Linux release artifacts are temporarily deferred while the
Tauri/Wry GTK runtime stack still carries a documented `glib 0.18.x` advisory
exception. The notes below are retained for internal deployment rehearsals and
source-built packages, not for the current public release download surface.

### Flatpak

A Flatpak bundle is available for distributions with Flatpak support:

```bash
flatpak install --user com.oneshim.client.flatpak
flatpak run com.oneshim.client
```

Config path inside Flatpak sandbox: `~/.config/oneshim/config.json`

### Debian/Ubuntu (.deb)

```bash
sudo dpkg -i oneshim_1.0.0_amd64.deb
sudo apt-get install -f   # resolve any dependencies
```

Runtime dependency: `libwebkit2gtk-4.1-0`

Config path: `~/.config/oneshim/config.json`

### RPM-based (RHEL, Fedora, openSUSE)

```bash
sudo rpm -i oneshim-1.0.0.x86_64.rpm
```

### systemd User Unit for Auto-Start

Deploy a systemd user unit to `/etc/skel/.config/systemd/user/oneshim.service` so it is copied for new users, or directly to `~/.config/systemd/user/oneshim.service` for existing users:

```ini
[Unit]
Description=Maekon Desktop Agent
After=graphical-session.target

[Service]
Type=simple
ExecStart=/usr/bin/oneshim
Restart=on-failure
RestartSec=5

[Install]
WantedBy=default.target
```

Enable and start:

```bash
systemctl --user daemon-reload
systemctl --user enable --now oneshim
```

---

## Per-Tenant Configuration

Maekon uses a JSON config file at the platform-specific path above. The following fields can be remotely managed per tenant. All fields use `#[serde(default)]`, so omitting a field applies the built-in default.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `server.base_url` | string | `http://localhost:8000` | Your connected Maekon server URL |
| `grpc.grpc_endpoint` | string | — | gRPC server endpoint (e.g., `https://grpc.example.com:50051`) |
| `grpc.use_grpc_auth` | bool | `false` | Enable gRPC for auth |
| `grpc.use_grpc_context` | bool | `false` | Enable gRPC for context upload |
| `privacy.pii_filter_level` | string | `"Standard"` | `"Off"`, `"Basic"`, `"Standard"`, `"Strict"` |
| `telemetry.enabled` | bool | `false` | Enable telemetry upload to server |
| `telemetry.crash_reports` | bool | `false` | Include crash reports in telemetry |
| `ai_provider.access_mode` | string | — | AI access mode for tenant |

### Example Managed Config

```json
{
  "server": {
    "base_url": "https://maekon.corp.example.com"
  },
  "grpc": {
    "grpc_endpoint": "https://grpc.corp.example.com:50051",
    "use_grpc_auth": true,
    "use_grpc_context": true,
    "use_tls": true
  },
  "privacy": {
    "pii_filter_level": "Strict"
  },
  "telemetry": {
    "enabled": false
  }
}
```

Fields not present in the deployed file retain their built-in defaults. This allows you to ship a minimal config that only overrides tenant-specific values.

### TLS Configuration

By default, TLS is enforced for all outbound connections. The REST client uses the system trust store and does not accept self-signed certificates by default.

For gRPC connections, the `grpc.use_tls` field (boolean, default `true`) controls whether TLS is required on the gRPC channel. Do not set this to `false` in production.

For internal CAs, distribute your CA certificate to the OS trust store using your MDM or GPO. The client inherits trust from the OS trust store; no per-field CA pin configuration is exposed in the JSON config.
