[English](./install.md) | [한국어](./install.ko.md)

# Installation Guide

This guide provides terminal-first installation for Maekon release binaries.

> Public GitHub Release assets for `pseudotop/maekon-client` are not published
> yet. The installer commands below require release assets and will work after
> the first public release is published. Until then, use the source build quick
> start in the repository README.

Compatibility note: release filenames, install script names, `MAEKON_*`
environment variables, and the `maekon` CLI command intentionally keep their
current names for installer, updater, and existing-user compatibility.

## Quick Install

### macOS / Linux

```bash
curl -fsSL -o /tmp/maekon-install.sh \
  https://raw.githubusercontent.com/pseudotop/maekon-client/main/scripts/install.sh
bash /tmp/maekon-install.sh
```

### Windows (PowerShell)

```powershell
$tmp = Join-Path $env:TEMP "maekon-install.ps1"
Invoke-WebRequest -UseBasicParsing `
  -Uri "https://raw.githubusercontent.com/pseudotop/maekon-client/main/scripts/install.ps1" `
  -OutFile $tmp
powershell -ExecutionPolicy Bypass -File $tmp
```

## Install a Specific Version

### macOS / Linux

```bash
MAEKON_VERSION=v0.0.4 bash /tmp/maekon-install.sh
```

### Windows

```powershell
powershell -ExecutionPolicy Bypass -File $tmp -Version v0.0.4
```

## Integrity Verification

- `scripts/install.sh` and `scripts/install.ps1` always verify `SHA-256` using release sidecars (`.sha256`).
- Ed25519 signature verification (`.sig`) is supported and can be enforced:
  - macOS/Linux: `--require-signature` or `MAEKON_REQUIRE_SIGNATURE=1`
  - Windows: `-RequireSignature`
- Signature verification requires Python + PyNaCl on the installation machine.
- Default update signing public key:
  - `GIdf7Wg4kvvvoT7jR0xwKLKna8hUR1kvowONbHbPz1E=`
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
- `--require-signature`

### Windows (`scripts/install.ps1`)

```powershell
powershell -ExecutionPolicy Bypass -File $tmp -?
```

Common parameters:

- `-Version <tag>` (default: `latest`)
- `-InstallDir <path>` (default: `%LOCALAPPDATA%\MAEKON\bin`)
- `-Repository <owner/name>` (default: `pseudotop/maekon-client`)
- `-BaseUrl <url>` (override release asset source; useful for local smoke/rehearsal)
- `-RequireSignature`

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
./scripts/install.sh
./scripts/uninstall.sh
```

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\install.ps1
powershell -ExecutionPolicy Bypass -File .\scripts\uninstall.ps1
```
