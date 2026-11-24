[English](./install.md) | [한국어](./install.ko.md)

# Installation Guide

This guide provides terminal-first installation for Maekon release binaries.

> This source snapshot targets prerelease `v0.0.1-rc.12`. Use the pinned
> commands only after the matching GitHub Release publishes its artifacts.
> GitHub's `latest` stable download URL is unavailable until the first stable
> release.

Compatibility note: release filenames, install script names, `MAEKON_*`
environment variables, and the `maekon` CLI command intentionally keep their
current names for installer, updater, and existing-user compatibility.

## Release Candidate Install (after publication)

### macOS / Linux

```bash
curl -fsSL -o /tmp/maekon-install.sh \
  https://raw.githubusercontent.com/pseudotop/maekon-client/main/scripts/install.sh
MAEKON_VERSION=v0.0.1-rc.12 bash /tmp/maekon-install.sh --require-signature
```

### Windows (PowerShell)

```powershell
$tmp = Join-Path $env:TEMP "maekon-install.ps1"
Invoke-WebRequest -UseBasicParsing `
  -Uri "https://raw.githubusercontent.com/pseudotop/maekon-client/main/scripts/install.ps1" `
  -OutFile $tmp
powershell -ExecutionPolicy Bypass -File $tmp -Version v0.0.1-rc.12 -RequireSignature
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
MAEKON_VERSION=v0.0.1-rc.12 bash /tmp/maekon-install.sh --require-signature
```

### Windows

```powershell
$tmp = Join-Path $env:TEMP "maekon-install.ps1"
Invoke-WebRequest -UseBasicParsing `
  -Uri "https://raw.githubusercontent.com/pseudotop/maekon-client/main/scripts/install.ps1" `
  -OutFile $tmp
powershell -ExecutionPolicy Bypass -File $tmp -Version v0.0.1-rc.12 -RequireSignature
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

Uninstall preserves your profile and credentials so that reinstalling does not
discard your data (policy choice 3, #11065). Package removal deletes installed
program files; Debian `purge` also prints a notice that user data remains. Package
scripts do not scan other users' homes or delete their keys. No separate profile
purge command is provided.

Default, unflavored profile locations:

| Platform | Settings | Data |
| --- | --- | --- |
| Linux | `~/.config/maekon` | `~/.local/share/maekon` |
| Windows | `%APPDATA%\maekon` | `%LOCALAPPDATA%\maekon\data` |
| macOS | `~/Library/Application Support/maekon` | `~/Library/Application Support/maekon/data` |

On Linux since rc.11, the durable database key is the owner-only `.db_key` file
inside the data directory; the kernel keyring is a cache. macOS Keychain and
Windows Credential Manager behavior is unchanged. Provider credentials may also
use an explicitly configured file. Flavored builds use `maekon-<flavor>` paths
and credential namespaces.

If you choose to remove profile files manually, stop Maekon first and review your
backups. Removing the Linux `.db_key` can make retained encrypted database copies
unreadable. Removing the profile directories does not erase OS credential entries,
separately configured files or backups, and does not establish secure erasure.
Linux kernel-keyring entries may remain until expiry or reboot; `secret-tool`
manages a different backend and is not a cleanup instruction for these entries.

The install verification lanes compare stopped-app profile files before and
after package removal. Their subsequent cleanup of the disposable test profile
measures filesystem removability separately; it does not prove credential-store
or backup erasure. This distinction also applies to #8685 criterion 7.

The clean QEMU VM lane selects Mesa `softpipe`: the default `llvmpipe` renderer
reproduces an AVX-related SIGSEGV under software CPU emulation (#13268). Its
removal control uses the same release with the renderer selection unset. Both
runs retain the default WebKit sandbox and Ubuntu user-namespace restrictions.
Both disable core dump collection inside the disposable VM and record the readback;
this lets the observer obtain the actual exit signal without a blocked core handler.
Receipts include the actual app PID/exit signal, HTTP timings and runtime logs;
a VM pass does not establish hardware GPU or general Linux desktop coverage.

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
MAEKON_VERSION=v0.0.1-rc.12 ./scripts/install.sh
./scripts/uninstall.sh
```

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\install.ps1 -Version v0.0.1-rc.12
powershell -ExecutionPolicy Bypass -File .\scripts\uninstall.ps1
```
