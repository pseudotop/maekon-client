# macOS Release Signing Runbook

This runbook defines required GitHub Actions secrets for signed + notarized macOS release artifacts.

## Why

- Gatekeeper blocks unsigned or non-notarized DMG/PKG builds.
- Maekon release workflow now signs app/pkg, notarizes app/dmg/pkg, and staples notarization tickets.

## Required GitHub Actions Secrets

- `MACOS_APP_CERT_P12_B64`: Base64-encoded `Developer ID Application` certificate (`.p12`)
- `MACOS_APP_CERT_PASSWORD`: Password for `MACOS_APP_CERT_P12_B64`
- `MACOS_INSTALLER_CERT_P12_B64`: Base64-encoded `Developer ID Installer` certificate (`.p12`)
- `MACOS_INSTALLER_CERT_PASSWORD`: Password for `MACOS_INSTALLER_CERT_P12_B64`
- `MACOS_APP_SIGNING_IDENTITY`: Exact signing identity string, for example `Developer ID Application: Example Inc (TEAMID)`
- `MACOS_INSTALLER_SIGNING_IDENTITY`: Exact installer identity string, for example `Developer ID Installer: Example Inc (TEAMID)`
- `MACOS_NOTARY_APPLE_ID`: Apple ID used for notarization
- `MACOS_NOTARY_TEAM_ID`: Apple Developer Team ID
- `MACOS_NOTARY_APP_PASSWORD`: App-specific password for `MACOS_NOTARY_APPLE_ID`

If any secret is missing, `build-macos-universal` fails fast.

## Asset Expectations

- `crates/oneshim-app/assets/icon.icns` must be a valid ICNS file.
- Source-of-truth logo asset is `assets/brand/logo-icon.svg`.
- Regenerate app icons via `./scripts/generate-app-icons.sh`.

## Local Preflight (optional)

```bash
file crates/oneshim-app/assets/icon.icns
codesign --verify --deep --strict --verbose=2 dist/Maekon.app
spctl --assess --type exec --verbose=4 dist/Maekon.app
spctl --assess --type open --verbose=4 dist/oneshim-macos-universal.dmg
spctl --assess --type install --verbose=4 dist/oneshim-macos-universal.pkg
```
