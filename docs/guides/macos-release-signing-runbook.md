# macOS Release Signing Runbook

This runbook defines required GitHub Actions secrets for signed + notarized macOS release artifacts.

## Why

- Gatekeeper blocks unsigned or non-notarized DMG/PKG builds.
- The release workflow signs the app bundle and PKG installer, submits both
  installers to Apple, and staples the notarization tickets in one job
  (`sign-notarize-macos` in `release.yml`), before anything is published. A
  release never exists with installers Gatekeeper would refuse.
- Final notarized bytes are the release source of truth. After stapling, the
  job records `notarization-final-byte-manifest.json` and generates `.sha256`
  and `.sig` sidecars over the stapled files. The publishing job then accepts
  that set only if `scripts/verify-release-final-bytes.py` passes, attests
  provenance, and creates the release.

## Approvals in a release run

A release run waits for the `release-signing` environment twice.

| Approval | Job | What it unlocks |
|----------|-----|-----------------|
| 1 | `verify-version` | The run itself: signed-tag, alert-state and keypair checks, then every build and smoke job |
| 2 | `sign-notarize-macos` | macOS signing certificates, Apple notarization credentials, and the update signing key |

Nothing after the second approval waits for a person. The installer smoke
launches the notarized DMG and PKG on a runner that holds no key, and
`create-release` publishes without an environment. It does not need a reviewer
because it cannot choose what to publish: the verifier requires every file to
carry a signature from an update key pinned in the release tree, the manifest
to name this tag and this run, and the DMG and PKG digests to equal the ones
the installer smoke launched. After publishing, the job downloads the release
and deletes it if the published bytes differ.

To rehearse the whole path without publishing, dispatch `release.yml` with
`rehearsal` enabled. It asks for the same two approvals, signs, notarizes and
smoke-tests, and stops before the release is created.

## `signingIdentity: null` — intentional, not a gap

`tauri.conf.json` sets `"signingIdentity": null` in the `bundle.macOS` section. This is
deliberate: it disables Tauri's built-in code-signing path. Maekon uses a custom signing
pipeline in `.github/workflows/release.yml` (the `sign-notarize-macos` job) instead,
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

### Signing secrets (fail-fast in `sign-notarize-macos` if absent)

| Secret | Purpose | Required |
|--------|---------|---------|
| `MACOS_APP_CERT_P12_B64` | Base64-encoded `Developer ID Application` certificate (`.p12`) | Yes — hard fail |
| `MACOS_APP_CERT_PASSWORD` | Password for `MACOS_APP_CERT_P12_B64` | Yes — hard fail |
| `MACOS_INSTALLER_CERT_P12_B64` | Base64-encoded `Developer ID Installer` certificate (`.p12`) | Yes — hard fail |
| `MACOS_INSTALLER_CERT_PASSWORD` | Password for `MACOS_INSTALLER_CERT_P12_B64` | Yes — hard fail |
| `MACOS_APP_SIGNING_IDENTITY` | Hint: exact `Developer ID Application: Example Inc (TEAMID)` string | Optional — auto-discovered from cert if absent |
| `MACOS_INSTALLER_SIGNING_IDENTITY` | Hint: exact `Developer ID Installer: Example Inc (TEAMID)` string | Optional — auto-discovered from cert if absent |

The "Validate release signing secret material" step in `release.yml` checks the four
**hard-fail** secrets before importing anything. If any is empty the job exits 1
immediately with a clear diagnostic message.

`MACOS_APP_SIGNING_IDENTITY` and `MACOS_INSTALLER_SIGNING_IDENTITY` are optional hints:
if absent, the workflow discovers the identity from the imported certificate via
`security find-identity`. If the resolved identity is still empty at that point, the job
fails with `"Unable to resolve Developer ID Application identity from imported certificate."`.

### Notarization secrets (checked in the same step)

| Secret | Purpose |
|--------|---------|
| `MACOS_NOTARY_APPLE_ID` | Apple ID email used for notarization |
| `MACOS_NOTARY_TEAM_ID` | Apple Developer Team ID |
| `MACOS_NOTARY_APP_PASSWORD` | App-specific password for `MACOS_NOTARY_APPLE_ID` |

"Validate release signing secret material" checks these three and
`UPDATE_SIGNING_PRIVATE_KEY_B64` together with the certificates. If any is
missing the job fails before it signs anything, so a run cannot spend an
approval and a signing pass only to stop at notarization.

Each secret is passed only to the step that uses it. Network installs
(`create-dmg`, the signing library) finish before the first of those steps.

## Behavior when secrets are missing

| Scenario | Outcome |
|----------|---------|
| `MACOS_APP_CERT_P12_B64` or `MACOS_APP_CERT_PASSWORD` absent | `sign-notarize-macos` fails at "Validate release signing secret material" before any signing attempt |
| `MACOS_INSTALLER_CERT_P12_B64` or `MACOS_INSTALLER_CERT_PASSWORD` absent | Same step, same outcome |
| `MACOS_APP_SIGNING_IDENTITY` absent | Identity auto-discovered from cert; no failure unless cert is also empty |
| Notarization secrets or update signing key absent | Same step, same outcome; nothing is signed and nothing is published |
| Apple rejects a submission, or notarization times out | `sign-notarize-macos` fails and nothing is published. The job leaves a `MAEKON_NOTARIZATION_FAILURE_V1` record and the private maintainer observer files the owner incident |
| Update signing key is not one the client pins | "Sign release artifacts (Ed25519)" refuses to sign; nothing is published |
| A final-set file is changed, unsigned, or signed by an unpinned key | `create-release` fails at "Verify final bytes against the pinned update keys" before the release is created |
| The published bytes differ from the verified set | `create-release` deletes the release and reports a final byte sidecar validation failure |
| Final notarized checksum or signature sidecar is stale | Release Guard deletes the invalid release and reports a final byte sidecar validation failure |

There is no silent skip path. Unsigned binaries cannot slip through a complete run of the
pipeline because the signing step calls `codesign --verify` after every signing operation.
Release Guard also verifies that the current downloadable macOS DMG/PKG bytes match their
`.sha256` sidecars, Ed25519 `.sig` sidecars, and `notarization-final-byte-manifest.json`.

## Recovering a failed run

- **The signing job failed** (certificate, Apple rejection, timeout): fix the
  cause and re-run the failed jobs of the same run. Nothing was published, so
  there is nothing to clean up. The re-run asks for the `release-signing`
  approval again and re-signs the same smoke-tested binaries: the job refuses a
  payload whose digest differs from the one the build uploaded.
- **The release is already published and its installers must be notarized
  again**: dispatch `notarize-macos-release-assets.yml` by hand with the tag.
  It no longer starts on its own. It waits for `macos-notarization` and
  `release-signing`, replaces the published DMG and PKG, and regenerates their
  sidecars and provenance. The replaced installers have not been through the
  release run's installer smoke, so prefer a new release candidate when the
  binaries themselves are in doubt.

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
6. Dispatch `release.yml` with `rehearsal` enabled to verify the new certs import,
   sign and notarize correctly before the next production release. A rehearsal
   publishes nothing.
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

- `.github/workflows/release.yml` — signs, notarizes and staples the app bundle, DMG and PKG (`sign-notarize-macos` job)
- `scripts/verify-release-final-bytes.py` — the check `create-release` runs before and after publishing
- `.github/workflows/notarize-macos-release-assets.yml` — manual repair path for an already published release
- `src-tauri/tauri.conf.json` — `signingIdentity: null` (intentional; see above)
- `src-tauri/assets/maekon.entitlements` — Hardened Runtime entitlements
