[English](./install.md) | [한국어](./install.ko.md)

# Installation Guide

This guide provides terminal-first installation for Maekon release binaries.

Compatibility note: release filenames, install script names, `ONESHIM_*`
environment variables, and the `oneshim` CLI command intentionally keep their
current names for installer, updater, and existing-user compatibility.

## Quick Install

### macOS / Linux

```bash
curl -fsSL -o /tmp/oneshim-install.sh \
  https://raw.githubusercontent.com/pseudotop/maekon-client/main/scripts/install.sh
bash /tmp/oneshim-install.sh
```

### Windows (PowerShell)

```powershell
$tmp = Join-Path $env:TEMP "oneshim-install.ps1"
Invoke-WebRequest -UseBasicParsing `
  -Uri "https://raw.githubusercontent.com/pseudotop/maekon-client/main/scripts/install.ps1" `
  -OutFile $tmp
powershell -ExecutionPolicy Bypass -File $tmp
```

## Install a Specific Version

### macOS / Linux

```bash
ONESHIM_VERSION=v0.0.4 bash /tmp/oneshim-install.sh
```

### Windows

```powershell
powershell -ExecutionPolicy Bypass -File $tmp -Version v0.0.4
```

## Integrity Verification

- `scripts/install.sh` and `scripts/install.ps1` always verify `SHA-256` using release sidecars (`.sha256`).
- Ed25519 signature verification (`.sig`) is supported and can be enforced:
  - macOS/Linux: `--require-signature` or `ONESHIM_REQUIRE_SIGNATURE=1`
  - Windows: `-RequireSignature`
- Signature verification requires Python + PyNaCl on the installation machine.
- Default update signing public key:
  - `GIdf7Wg4kvvvoT7jR0xwKLKna8hUR1kvowONbHbPz1E=`
- Override key when rotated:
  - `ONESHIM_UPDATE_PUBLIC_KEY=<base64-ed25519-public-key>`

## Script Options

### macOS / Linux (`scripts/install.sh`)

```bash
bash /tmp/oneshim-install.sh --help
```

Common options:

- `--version <tag>` (default: `latest`)
- `--install-dir <path>` (default: `~/.local/bin`)
- `--repo <owner/name>` (default: `pseudotop/maekon-client`)
- `--base-url <url>` (override release asset source; useful for local smoke/rehearsal)
- `--require-signature`

### Windows (`scripts/install.ps1`)

```powershell
powershell -ExecutionPolicy Bypass -File $tmp -?
```

Common parameters:

- `-Version <tag>` (default: `latest`)
- `-InstallDir <path>` (default: `%LOCALAPPDATA%\ONESHIM\bin`)
- `-Repository <owner/name>` (default: `pseudotop/maekon-client`)
- `-BaseUrl <url>` (override release asset source; useful for local smoke/rehearsal)
- `-RequireSignature`

## Uninstall

### macOS / Linux

```bash
curl -fsSL -o /tmp/oneshim-uninstall.sh \
  https://raw.githubusercontent.com/pseudotop/maekon-client/main/scripts/uninstall.sh
bash /tmp/oneshim-uninstall.sh
```

### Windows

```powershell
$tmp = Join-Path $env:TEMP "oneshim-uninstall.ps1"
Invoke-WebRequest -UseBasicParsing `
  -Uri "https://raw.githubusercontent.com/pseudotop/maekon-client/main/scripts/uninstall.ps1" `
  -OutFile $tmp
powershell -ExecutionPolicy Bypass -File $tmp
```

## Local Repository Usage

If you already cloned this repository:

```bash
./scripts/install.sh
./scripts/uninstall.sh
```

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\install.ps1
powershell -ExecutionPolicy Bypass -File .\scripts\uninstall.ps1
```
