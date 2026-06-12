[English](./install.md) | [한국어](./install.ko.md)

# Installation Guide

This guide provides terminal-first installation for Maekon release binaries.

> The current public binary release is `v0.0.1-rc.6`, published as a
> prerelease. GitHub's `latest` stable download URL is not available until the
> first stable release, so current prerelease installs must pin the version.

Compatibility note: release filenames, install script names, `MAEKON_*`
environment variables, and the `maekon` CLI command intentionally keep their
current names for installer, updater, and existing-user compatibility.

## Current Prerelease Install

### macOS / Linux

```bash
curl -fsSL -o /tmp/maekon-install.sh \
  https://raw.githubusercontent.com/pseudotop/maekon-client/main/scripts/install.sh
MAEKON_VERSION=v0.0.1-rc.6 bash /tmp/maekon-install.sh --require-signature
```

### Windows (PowerShell)

```powershell
$tmp = Join-Path $env:TEMP "maekon-install.ps1"
Invoke-WebRequest -UseBasicParsing `
  -Uri "https://raw.githubusercontent.com/pseudotop/maekon-client/main/scripts/install.ps1" `
  -OutFile $tmp
powershell -ExecutionPolicy Bypass -File $tmp -Version v0.0.1-rc.6 -RequireSignature
```

## Latest Stable Install

After the first stable release is published, the unpinned installer defaults to
GitHub's `latest` stable release:

### macOS / Linux

```bash
curl -fsSL https://raw.githubusercontent.com/pseudotop/maekon-client/main/scripts/install.sh | bash -s -- --require-signature
```

### Windows

```powershell
$tmp = Join-Path $env:TEMP "maekon-install.ps1"
Invoke-WebRequest -UseBasicParsing `
  -Uri "https://raw.githubusercontent.com/pseudotop/maekon-client/main/scripts/install.ps1" `
  -OutFile $tmp
powershell -ExecutionPolicy Bypass -File $tmp -RequireSignature
```

## Install a Specific Version

### macOS / Linux

```bash
curl -fsSL -o /tmp/maekon-install.sh \
  https://raw.githubusercontent.com/pseudotop/maekon-client/main/scripts/install.sh
MAEKON_VERSION=v0.0.1-rc.6 bash /tmp/maekon-install.sh --require-signature
```

### Windows

```powershell
$tmp = Join-Path $env:TEMP "maekon-install.ps1"
Invoke-WebRequest -UseBasicParsing `
  -Uri "https://raw.githubusercontent.com/pseudotop/maekon-client/main/scripts/install.ps1" `
  -OutFile $tmp
powershell -ExecutionPolicy Bypass -File $tmp -Version v0.0.1-rc.6 -RequireSignature
```

## Integrity Verification

- `scripts/install.sh` and `scripts/install.ps1` always verify `SHA-256` using release sidecars (`.sha256`).
- Ed25519 signature verification (`.sig`) is enforced by default for public installs:
  - macOS/Linux: `--require-signature` or default `MAEKON_REQUIRE_SIGNATURE=1`
  - Windows: `-RequireSignature` or default `MAEKON_REQUIRE_SIGNATURE=1`
- Developer/test unsigned rehearsals must opt out explicitly:
  - macOS/Linux: `--allow-unsigned` or `MAEKON_REQUIRE_SIGNATURE=0`
  - Windows: `-AllowUnsigned` or `MAEKON_REQUIRE_SIGNATURE=0`
- Signature verification requires Python + PyNaCl on the installation machine.
- Default update signing public key:
  - `fPiU9KchUIXZ7qOcjJIVp+W8rsO/WI7yStD+AiNuYvw=`
- Override key when rotated:
  - `MAEKON_UPDATE_PUBLIC_KEY=<base64-ed25519-public-key>`

## Script Options

### macOS / Linux (`scripts/install.sh`)

```bash
bash /tmp/maekon-install.sh --help
```

Common options:

- `--version <tag>` (default: `latest`)
- `--install-dir <path>` (default: `~/.local/bin`)
- `--repo <owner/name>` (default: `pseudotop/maekon-client`)
- `--base-url <url>` (override release asset source; useful for local smoke/rehearsal)
- `--require-signature` (default public behavior)
- `--allow-unsigned` (developer/test override only)

### Windows (`scripts/install.ps1`)

```powershell
powershell -ExecutionPolicy Bypass -File $tmp -?
```

Common parameters:

- `-Version <tag>` (default: `latest`)
- `-InstallDir <path>` (default: `%LOCALAPPDATA%\MAEKON\bin`)
- `-Repository <owner/name>` (default: `pseudotop/maekon-client`)
- `-BaseUrl <url>` (override release asset source; useful for local smoke/rehearsal)
- `-RequireSignature` (default public behavior)
- `-AllowUnsigned` (developer/test override only)

## Uninstall

### macOS / Linux

```bash
curl -fsSL -o /tmp/maekon-uninstall.sh \
  https://raw.githubusercontent.com/pseudotop/maekon-client/main/scripts/uninstall.sh
bash /tmp/maekon-uninstall.sh
```

### Windows

```powershell
$tmp = Join-Path $env:TEMP "maekon-uninstall.ps1"
Invoke-WebRequest -UseBasicParsing `
  -Uri "https://raw.githubusercontent.com/pseudotop/maekon-client/main/scripts/uninstall.ps1" `
  -OutFile $tmp
powershell -ExecutionPolicy Bypass -File $tmp
```

## Local Repository Usage

If you already cloned this repository:

```bash
MAEKON_VERSION=v0.0.1-rc.6 ./scripts/install.sh
./scripts/uninstall.sh
```

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\install.ps1 -Version v0.0.1-rc.6
powershell -ExecutionPolicy Bypass -File .\scripts\uninstall.ps1
```
