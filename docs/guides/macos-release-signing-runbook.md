# macOS Release Signing Runbook

This runbook defines required GitHub Actions secrets for signed + notarized macOS release artifacts.

## Why

- Gatekeeper blocks unsigned or non-notarized DMG/PKG builds.
- Maekon release workflow signs the app bundle and PKG installer, then the separate
  notarization workflow (`notarize-macos-release-assets.yml`) submits them to Apple and
  staples the notarization ticket.
- Final notarized bytes are the release source of truth. After stapling, the
  workflow records `notarization-final-byte-manifest.json`, passes the DMG/PKG
  to a separate `release-signing` job, regenerates `.sha256` and `.sig`
  sidecars, attests provenance, and only then republishes the release assets.

## `signingIdentity: null` — intentional, not a gap

`tauri.conf.json` sets `"signingIdentity": null` in the `bundle.macOS` section. This is
deliberate: it disables Tauri's built-in code-signing path. Maekon uses a custom signing
pipeline in `.github/workflows/release.yml` (the `build-macos-universal` job) instead,
which imports the certificate into a fresh ephemeral keychain and calls `codesign` /
`productsign` directly. This gives full control over signing flags, entitlements, and
identity resolution while keeping `src-tauri/tauri.conf.json` unchanged between dev and
CI builds.

If Tauri's built-in signer were used (`signingIdentity` set to a non-null string),
`tauri bundle` would call `codesign` during the local build command, which would fail on
developer machines that do not have the `Developer ID Application` certificate installed.
Setting it to `null` makes local builds succeed without requiring the release certificate.

The environment variable `APPLE_SIGNING_IDENTITY` referenced in older Tauri documentation
is **not used** by this project. The CI workflow uses `MACOS_APP_SIGNING_IDENTITY` instead.

## Required GitHub Actions Secrets

### Signing secrets (fail-fast in `build-macos-universal` if absent)

| Secret | Purpose | Required |
|--------|---------|---------|
| `MACOS_APP_CERT_P12_B64` | Base64-encoded `Developer ID Application` certificate (`.p12`) | Yes — hard fail |
| `MACOS_APP_CERT_PASSWORD` | Password for `MACOS_APP_CERT_P12_B64` | Yes — hard fail |
| `MACOS_INSTALLER_CERT_P12_B64` | Base64-encoded `Developer ID Installer` certificate (`.p12`) | Yes — hard fail |
| `MACOS_INSTALLER_CERT_PASSWORD` | Password for `MACOS_INSTALLER_CERT_P12_B64` | Yes — hard fail |
| `MACOS_APP_SIGNING_IDENTITY` | Hint: exact `Developer ID Application: Example Inc (TEAMID)` string | Optional — auto-discovered from cert if absent |
| `MACOS_INSTALLER_SIGNING_IDENTITY` | Hint: exact `Developer ID Installer: Example Inc (TEAMID)` string | Optional — auto-discovered from cert if absent |

The "Validate macOS signing secret material" step in `release.yml` checks the four
**hard-fail** secrets before importing anything. If any is empty the job exits 1
immediately with a clear diagnostic message.

`MACOS_APP_SIGNING_IDENTITY` and `MACOS_INSTALLER_SIGNING_IDENTITY` are optional hints:
if absent, the workflow discovers the identity from the imported certificate via
`security find-identity`. If the resolved identity is still empty at that point, the job
fails with `"Unable to resolve Developer ID Application identity from imported certificate."`.

### Notarization secrets (checked in `notarize-macos-release-assets.yml`)

| Secret | Purpose |
|--------|---------|
| `MACOS_NOTARY_APPLE_ID` | Apple ID email used for notarization |
| `MACOS_NOTARY_TEAM_ID` | Apple Developer Team ID |
| `MACOS_NOTARY_APP_PASSWORD` | App-specific password for `MACOS_NOTARY_APPLE_ID` |

These are validated by the "Validate notarization secrets" step. If any is missing the
notarization job fails fast before any asset is downloaded.

## Behavior when secrets are missing

| Scenario | Outcome |
|----------|---------|
| `MACOS_APP_CERT_P12_B64` or `MACOS_APP_CERT_PASSWORD` absent | `build-macos-universal` fails at "Validate macOS signing secret material" before any signing attempt |
| `MACOS_INSTALLER_CERT_P12_B64` or `MACOS_INSTALLER_CERT_PASSWORD` absent | Same step, same outcome |
| `MACOS_APP_SIGNING_IDENTITY` absent | Identity auto-discovered from cert; no failure unless cert is also empty |
| Notarization secrets absent | `notarize-macos-release-assets.yml` fails at "Validate notarization secrets"; release acceptance remains blocked until notarized final bytes are published with fresh sidecars and provenance |
| Final notarized checksum or signature sidecar is stale | Release Guard deletes the invalid release and reports a final byte sidecar validation failure |

There is no silent skip path. Unsigned binaries cannot slip through a complete run of the
pipeline because the signing step calls `codesign --verify` after every signing operation.
Release Guard also verifies that the current downloadable macOS DMG/PKG bytes match their
`.sha256` sidecars, Ed25519 `.sig` sidecars, and `notarization-final-byte-manifest.json`.

## Secret rotation procedure

### Signing certificate rotation (annual or on compromise)

1. Generate a new `Developer ID Application` and `Developer ID Installer` certificate pair
   in Apple Developer Portal for the Maekon team account.
2. Export each as a `.p12` file with a strong password.
3. Base64-encode: `base64 -i new-app.p12 | pbcopy` (macOS).
4. Update the four GitHub Actions secrets in the repository settings:
   `MACOS_APP_CERT_P12_B64`, `MACOS_APP_CERT_PASSWORD`,
   `MACOS_INSTALLER_CERT_P12_B64`, `MACOS_INSTALLER_CERT_PASSWORD`.
5. Optionally update `MACOS_APP_SIGNING_IDENTITY` / `MACOS_INSTALLER_SIGNING_IDENTITY`
   if the team name or Team ID changed.
6. Run a dry-run release (`workflow_dispatch`) against a test tag to verify the new certs
   import and sign correctly before the next production release.
7. Revoke the old certificate in Apple Developer Portal only after the dry-run succeeds.

### Notarization credential rotation (annual or on Apple ID compromise)

1. Log in to appleid.apple.com and generate a new app-specific password for the Maekon
   notarization Apple ID.
2. Update `MACOS_NOTARY_APP_PASSWORD` in GitHub Actions secrets.
3. If the Apple ID itself is being replaced: update `MACOS_NOTARY_APPLE_ID` and
   `MACOS_NOTARY_TEAM_ID` as well; ensure the new Apple ID has accepted the Apple
   Developer Program license agreement.
4. Revoke the old app-specific password in appleid.apple.com.

## Asset Expectations

- `src-tauri/icons/icon.icns` must be a valid ICNS file (validated by the
  "Validate macOS icon asset format" step in `release.yml`).
- `notarization-final-byte-manifest.json` must be published with the final
  notarized bytes and must link each macOS installer checksum to the notary log
  generated for that asset.
- Source-of-truth logo asset is `assets/brand/logo-icon.svg`.
- Regenerate app icons via `./scripts/generate-app-icons.sh`.

## Local Preflight (optional)

```bash
file src-tauri/icons/icon.icns
./scripts/verify-macos-app-bundle.sh dist/Maekon.app
spctl --assess --type exec --verbose=4 dist/Maekon.app
spctl --assess --type open --verbose=4 dist/maekon-macos-universal.dmg
spctl --assess --type install --verbose=4 dist/maekon-macos-universal.pkg
```

Release-candidate SemVer strings stay in `CFBundleShortVersionString`. The
machine build field must be derived with `scripts/macos-bundle-version.py`
(for example, `0.0.1-rc.10` becomes `0.0.1fc10`) before signing. The verifier
also rejects signatures whose `Info.plist` is not bound, whose entitlements
blob cannot be decoded, or whose architecture-specific signatures fail.

## Related

- `.github/workflows/release.yml` — signs app bundle and PKG (`build-macos-universal` job, lines 704+)
- `.github/workflows/notarize-macos-release-assets.yml` — submits to Apple, staples ticket
- `src-tauri/tauri.conf.json` — `signingIdentity: null` (intentional; see above)
- `src-tauri/assets/maekon.entitlements` — Hardened Runtime entitlements
