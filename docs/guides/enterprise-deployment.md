# Enterprise Deployment Guide

This guide covers mass deployment of MAEKON to managed fleets using MDM, Group Policy, and Linux package managers. It is intended for IT administrators and enterprise architects.

See also: [SECURITY.md](../../SECURITY.md)

---

## Table of Contents

- [Network Requirements](#network-requirements)
- [Data Residency](#data-residency)
- [Cloud Speech-to-Text (STT) Egress Control](#cloud-speech-to-text-stt-egress-control)
- [macOS MDM Deployment](#macos-mdm-deployment)
- [Windows GPO Deployment](#windows-gpo-deployment)
- [Linux Deployment](#linux-deployment)
- [Per-Tenant Configuration](#per-tenant-configuration)

---

## Network Requirements

MAEKON requires outbound access from endpoints to:

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

- macOS: `~/Library/Application Support/maekon/data/`
- Windows: `%LOCALAPPDATA%\maekon\data\`
- Linux: `~/.local/share/maekon/`

Data is only transmitted to your connected Maekon server when `telemetry.enabled` is `true` (fresh-install default: `true`), telemetry consent is valid, and the binary was built with the `telemetry` feature. PII is filtered on-device before any upload, controlled by `privacy.pii_filter_level` (default: `"Standard"`).

Disabling telemetry keeps all data on the device and prevents any outbound data transfer. This satisfies data residency requirements for regions that prohibit cross-border data flows.

---

## Cloud Speech-to-Text (STT) Egress Control

Voice capture is **off by default** (`audio.enabled = false`). When a user enables it, transcription runs **locally** by default (`audio.stt_provider = "local"`, Whisper on-device). Cloud STT is a **user-directed egress**: it activates only when a user sets `audio.stt_provider = "cloud"` **and** supplies `audio.cloud_api_key`. The default cloud endpoint (`audio.cloud_stt_endpoint`) is **OpenAI's API (`https://api.openai.com/v1/audio/transcriptions`), processed in the United States** — a cross-border transfer (GDPR Chapter V) for EU data subjects. Review [OpenAI's API data usage and retention policy](https://openai.com/policies/api-data-usage-policies) before enabling, and reflect the chosen provider's location/retention in your own RoPA and DPA.

For managed fleets, enforce a fleet-wide policy with `audio.cloud_stt_policy`, independent of whether any individual user enters an API key:

| `audio.cloud_stt_policy` | Effect |
|--------------------------|--------|
| `"allow"` (default) | Cloud STT permitted when the user configures it (preserves current consumer behavior). |
| `"require_admin_approval"` | Cloud STT egress is blocked until an admin-approval channel approves it. Until that channel ships, this behaves as a block (fail-safe). |
| `"disabled"` | Cloud STT hard-disabled fleet-wide. Raw audio never leaves the device; STT falls back to the local Whisper provider if available. |

To guarantee "no raw audio leaves the device to any third party" across the fleet, deploy:

```json
{
  "audio": {
    "cloud_stt_policy": "disabled"
  }
}
```

Enforcement is applied at STT provider construction — both the startup path (`src-tauri/src/app_runtime_launch/audio_wiring.rs`) and the live config-reload path (`src-tauri/src/commands/audio.rs`): when the policy is not `"allow"`, the cloud STT provider is never constructed regardless of `cloud_api_key`, and the agent logs a managed-policy block. This is independent of the build-time `cloud-stt` feature gate (which is a separate compile-time control).

---

## macOS MDM Deployment

### Supported MDM Platforms

Jamf Pro, Mosyle Business, Kandji, and any MDM that supports `.pkg` distribution and `LaunchAgent` management.

### 1. Package the Application

Build a notarized `.dmg` from the release pipeline (see [CI Transparency](ci-transparency.md)). Wrap the `.dmg` content in a flat `.pkg` using `pkgbuild`:

```bash
pkgbuild \
  --root /path/to/MAEKON.app \
  --install-location /Applications \
  --identifier com.maekon.app \
  --version 1.0.0 \
  MAEKON-1.0.0.pkg
```

Sign and notarize the `.pkg` with your Apple Developer ID Installer certificate before distributing via MDM.

### 2. LaunchAgent for Auto-Start

To start MAEKON at user login, deploy a `LaunchAgent` plist via MDM:

**File path on endpoint**: `~/Library/LaunchAgents/com.maekon.app.plist`

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.maekon.app</string>
    <key>ProgramArguments</key>
    <array>
        <string>/Applications/MAEKON.app/Contents/MacOS/maekon</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <false/>
    <key>StandardOutPath</key>
    <string>/tmp/maekon.out.log</string>
    <key>StandardErrorPath</key>
    <string>/tmp/maekon.err.log</string>
</dict>
</plist>
```

Load on first deployment: `launchctl load ~/Library/LaunchAgents/com.maekon.app.plist`

### 3. Managed Preferences (Jamf/Kandji)

Deploy a pre-configured `config.json` to each endpoint via MDM file distribution. Target path:

```
~/Library/Application Support/maekon/config.json
```

The config file is JSON. Use the per-tenant configuration fields listed in the [Per-Tenant Configuration](#per-tenant-configuration) section below. Note this `config.json` sets *defaults* the user can still change; to **enforce** a value, use the [Managed Policy Lock (`managed.json`)](#managed-policy-lock-managedjson) instead.

---

## Windows GPO Deployment

### 1. Silent MSI Install

Build the Windows installer from the release pipeline. The installer is a standard `.msi`. Silent installation via GPO:

```powershell
msiexec /i MAEKON-1.0.0.msi /quiet /norestart ALLUSERS=1
```

Or via Group Policy software deployment:
1. Copy `.msi` to a network share accessible to target computers.
2. In GPMC: Computer Configuration > Policies > Software Settings > Software Installation > New Package.
3. Select "Assigned" deployment.

### 2. Registry-Based Configuration

Pre-seed the configuration by deploying `config.json` via a GPO Preference (Files item):

**Source**: `\\server\share\maekon\config.json`
**Destination**: `%APPDATA%\maekon\config.json`
**Action**: Replace

Alternatively, use a startup script:

```powershell
$configDir = "$env:APPDATA\maekon"
if (-not (Test-Path $configDir)) { New-Item -ItemType Directory $configDir }
Copy-Item "\\server\share\maekon\config.json" "$configDir\config.json" -Force
```

### 3. ADMX Template

Reference Group Policy templates are published under [`docs/gpo/`](../gpo/):

- [`docs/gpo/maekon.admx`](../gpo/maekon.admx) — policy definitions for the six lockable Maekon settings
- [`docs/gpo/en-US/maekon.adml`](../gpo/en-US/maekon.adml) — English display strings

Install by copying `maekon.admx` into the Central Store (`%SystemRoot%\PolicyDefinitions` or the domain SYSVOL Central Store) and `maekon.adml` into the matching `en-US\` subfolder. The policies then appear under **Computer Configuration → Administrative Templates → Maekon**, writing values under `HKLM\Software\Policies\Maekon`.

> **Status (E20-45 #4837):** these are *reference* templates. (1) They were authored on a non-Windows host — validate with `admchk.exe` (or load in `gpedit.msc`) before production rollout. (2) The registry values they set are **not yet auto-translated** into `managed.json`; a registry→`managed.json` bridge is future work. Until that ships, the **enforced** policy path is the `managed.json` file distributed via MDM (see [Managed Policy Lock](#managed-policy-lock-managedjson) below) — the ADMX template documents the same six-field policy surface for GPO authoring.

### 4. Auto-Start via Registry

To start MAEKON at user login without relying on the built-in autostart, add a Run key via GPO Preferences:

- **Key**: `HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Run`
- **Value name**: `MAEKON`
- **Value data**: `"C:\Program Files\MAEKON\maekon.exe"`

---

## Linux Deployment

### Flatpak

A Flatpak bundle is available for distributions with Flatpak support:

```bash
flatpak install --user com.maekon.app.flatpak
flatpak run com.maekon.app
```

Config path inside Flatpak sandbox: `~/.config/maekon/config.json`

### Debian/Ubuntu (.deb)

```bash
sudo dpkg -i maekon_1.0.0_amd64.deb
sudo apt-get install -f   # resolve any dependencies
```

Runtime dependency: `libwebkitgtk-6.0-4`

Config path: `~/.config/maekon/config.json`

### RPM-based (RHEL, Fedora, openSUSE)

```bash
sudo rpm -i maekon-1.0.0.x86_64.rpm
```

### systemd User Unit for Auto-Start

Deploy a systemd user unit to `/etc/skel/.config/systemd/user/maekon.service` so it is copied for new users, or directly to `~/.config/systemd/user/maekon.service` for existing users:

```ini
[Unit]
Description=MAEKON Desktop Agent
After=graphical-session.target

[Service]
Type=simple
ExecStart=/usr/bin/maekon
Restart=on-failure
RestartSec=5

[Install]
WantedBy=default.target
```

Enable and start:

```bash
systemctl --user daemon-reload
systemctl --user enable --now maekon
```

---

## Per-Tenant Configuration

MAEKON uses a JSON config file at the platform-specific path above. The following fields can be remotely managed per tenant. All fields use `#[serde(default)]`, so omitting a field applies the built-in default.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `server.base_url` | string | `http://localhost:8000` | Your connected Maekon server URL |
| `grpc.grpc_endpoint` | string | — | gRPC server endpoint (e.g., `https://grpc.example.com:50051`) |
| `grpc.use_grpc_auth` | bool | `false` | Enable gRPC for auth |
| `grpc.use_grpc_context` | bool | `false` | Enable gRPC for context upload |
| `privacy.pii_filter_level` | string | `"Standard"` | `"Off"`, `"Basic"`, `"Standard"`, `"Strict"` |
| `audio.cloud_stt_policy` | string | `"allow"` | `"allow"`, `"require_admin_approval"`, `"disabled"` — fleet gate for cloud STT raw-audio egress (see [Cloud STT Egress Control](#cloud-speech-to-text-stt-egress-control)) |
| `telemetry.enabled` | bool | `true` | Enable telemetry intent; upload still requires valid telemetry consent and the `telemetry` Cargo feature |
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

> ⚠️ The per-tenant `config.json` above is **not locked** — it lives at the
> per-user config path the app itself writes back to, so a local user can change
> any of these values. To *enforce* a policy the user cannot override, use the
> Managed Policy Lock below.

### Managed Policy Lock (`managed.json`)

For values that must be **enforced** (the local user cannot override them — e.g.
on a shared RDP/Citrix host), deploy a read-only `managed.json` policy file to
the system-wide, admin-owned location. Every field present in `managed.json` is
**locked**: any user attempt to change it (via the dashboard settings page or the
local IPC) is rejected with a "locked by your administrator" message, and the
write chokepoint re-clamps the value by construction on every path (settings API,
backup restore, scheduler) — so the lock cannot be bypassed.

**Location** (deploy with root/admin-only write permissions; the OS file ACL —
not the app — is the tamper barrier):

| Platform | Path |
|----------|------|
| macOS | `/Library/Application Support/maekon/managed.json` |
| Windows | `%ProgramData%\maekon\managed.json` |
| Linux | `/etc/maekon/managed.json` |

(Override the path for testing/staging with the `MAEKON_MANAGED_CONFIG_PATH`
environment variable.)

**JSON Schema** (E20-45 #4837): a published schema for `managed.json` lives at
[`docs/contracts/managed.schema.json`](../contracts/managed.schema.json). Point
your editor (VS Code: add a `$schema` key or a `json.schemas` mapping) or your MDM
config-authoring tooling at it for validation + autocomplete of the lockable
fields and their enum values. The schema mirrors the runtime contract: known
fields are strictly typed and enum-constrained, unknown keys are tolerated for
forward-compatibility, and `schema_version` is pinned to the version the client
supports (a newer version is rejected fail-closed at load). A unit test
(`published_json_schema_covers_every_lockable_field`) keeps the schema in lockstep
with the lockable allowlist — adding a new locked field without updating the schema
fails the build.

**Lockable fields** (MVP allowlist — each is optional; omit to leave it
user-controlled):

| Field | Type | Effect when locked |
|-------|------|--------------------|
| `privacy.pii_filter_level` | `"Off"`/`"Basic"`/`"Standard"`/`"Strict"` | Forces the PII filter level |
| `telemetry.enabled` | bool | Forces telemetry on/off |
| `telemetry.crash_reports` | bool | Forces crash-report inclusion |
| `vision.capture_enabled` | bool | Forces screen capture on/off |
| `audio.cloud_stt_policy` | `"allow"`/`"require_admin_approval"`/`"disabled"` | Forces the cloud-STT egress policy |
| `update.enabled` | bool | Locking `false` is a fleet update kill-switch |

**Example** — lock privacy/telemetry/capture for a regulated fleet:

```json
{
  "schema_version": 1,
  "privacy": { "pii_filter_level": "Strict" },
  "telemetry": { "enabled": false, "crash_reports": false },
  "vision": { "capture_enabled": false }
}
```

**Behavior**

- **Absent file** ⇒ no policy ⇒ normal (consumer) operation.
- **Present but malformed / bad enum / future `schema_version`** ⇒ **fail-closed**:
  the client refuses to start unmanaged (a broken policy file means the admin
  *intended* locks; silently ignoring it would be fail-open on privacy/telemetry).
- Unknown extra keys are tolerated, so a newer policy file does not brick an
  older client.
- Locks take effect at next launch (`managed.json` is read once at startup).

> ADMX templates (managed via GPO) and staged-rollout cohorts build on this layer
> and are tracked separately. This MVP ships the `managed.json` file + enforcement
> only.

### Remote kill-switch (disable updates via MDM)

If a bad release ships, or you need to freeze a fleet at a known-good version,
lock `update.enabled` to `false` in `managed.json`. This is the **remote update
kill-switch**: it disables the in-app updater for every managed device, and the
local user cannot turn it back on.

Deploy this `managed.json` to the admin-owned [location](#managed-policy-lock-managedjson)
above:

```json
{
  "schema_version": 1,
  "update": { "enabled": false }
}
```

**What it does**

- The effective `update.enabled` is forced to `false` even if the user's
  `config.json` (or the [example managed config](#example-managed-config)
  default) sets it to `true` — the managed clamp wins.
- The startup update check is **never spawned** (`UpdateRuntimeBuilder::build_and_spawn`
  in `src-tauri/src/update_runtime.rs` only spawns the loop when
  `config.update.enabled` is true).
- The update coordinator stays **Idle** with the message
  `"Update feature is disabled"` and never polls GitHub Releases
  (`run_update_coordinator` in `src-tauri/src/update_coordinator/mod.rs` returns
  early when `!config.enabled`).
- Any user/web attempt to re-enable updates (dashboard settings page or local
  IPC) is **rejected** with a "locked by your administrator" message, and the
  config write chokepoint re-clamps the value to `false` on every path.

**Operational notes**

- The kill-switch takes effect at the **next client launch** (`managed.json` is
  read once at startup). To take effect immediately on already-running devices,
  push the policy *and* restart the agent (e.g. via your MDM's restart/relaunch
  command).
- To **re-enable** updates, set `"enabled": true` (or remove the `update` block
  entirely to return updates to user control), then relaunch.
- The kill-switch only disables the *update mechanism*. It does not roll devices
  back to a prior version — combine with your MDM's app-distribution channel if
  you need to push a specific good build.
- This is a binary on/off control. Targeting updates to a subset of the fleet
  (staged-rollout cohorts) is a separate, server-side capability tracked
  independently and is **not** part of this `managed.json` layer.

### TLS Configuration

By default, TLS is enforced for all outbound connections. The REST client uses the system trust store and does not accept self-signed certificates by default.

For gRPC connections, the `grpc.use_tls` field (boolean, default `true`) controls whether TLS is required on the gRPC channel. Do not set this to `false` in production.

For internal CAs, distribute your CA certificate to the OS trust store using your MDM or GPO. The client inherits trust from the OS trust store; no per-field CA pin configuration is exposed in the JSON config.
