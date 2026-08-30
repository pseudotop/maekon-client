# Windows Authenticode Activation Runbook

## Current state

The Windows signing pipeline is `prepared_not_active`. The repository pins the
required artifacts, final-byte order, OIDC subject, and publisher/timestamp
verification contract, but it does not claim signing before a legal-entity
Public Trust identity and certificate profile exist.

The canonical policy is `supply-chain/windows-authenticode-policy.json`.
ADR-005 assigns the certificate and external signing service to DevOps/release
engineering.

## Preparation rules

- Never store a PFX, private key, or certificate password in the repository or
  GitHub Actions secrets.
- Use Azure Artifact Signing Public Trust with GitHub OIDC.
- Bind the federated subject exactly to
  `repo:pseudotop/maekon-client:environment:release-signing`.
- Scope `Artifact Signing Certificate Profile Signer` to the certificate profile.
- Do not change the policy `enforcement_state` to `active` before Public Trust
  identity validation is complete.
- Use SHA-256 and an RFC 3161 timestamp.

## External setup

1. Prepare an Azure billing profile that matches the legal entity.
2. Create an Artifact Signing Basic account in Korea Central.
3. Complete organization Public Identity Validation.
4. Create a `Public Trust` certificate profile.
5. Create an Entra application and a federated credential for the protected
   `release-signing` environment subject above.
6. Assign the Signer role at certificate-profile scope.
7. Configure these non-key values in the public repository's protected
   `release-signing` environment:

- `AZURE_TENANT_ID`
- `AZURE_SUBSCRIPTION_ID`
- `AZURE_CLIENT_ID`
- `WINDOWS_ARTIFACT_SIGNING_ENDPOINT`
- `WINDOWS_ARTIFACT_SIGNING_ACCOUNT`
- `WINDOWS_ARTIFACT_SIGNING_PROFILE`

Keep those values in the protected `release-signing` environment. Manage the
exact publisher subject and activation state as reviewed source fields in
`supply-chain/windows-authenticode-policy.json`, not as mutable environment
switches.

## Final-byte order to preserve during activation

1. Sign `maekon.exe` and `maekon-sandbox-worker.exe`.
2. Build the ZIP, Korean MSI, English MSI, and NSIS from the signed executables.
3. Sign both MSI files and the NSIS setup executable.
4. Verify publisher, RFC 3161 timestamp, and WinTrust for every executable and
   installer.
5. Generate SHA-256 from the verified final bytes.
6. Generate the existing Ed25519 updater signatures and provenance.
7. Publish release assets.

Authenticode establishes the Windows publisher identity. The Ed25519 `.sig`
establishes Maekon updater integrity. Neither replaces the other.

## Pre-activation rehearsal

Use an exact-SHA rehearsal that creates no tag or Release and prove:

- missing signing service access or an incorrect OIDC subject fails;
- `scripts/verify-windows-authenticode.ps1 -Mode Required` rejects unsigned,
  wrong-publisher, untimestamped, and modified files;
- the installed application and sandbox-worker carry the same publisher;
- checksums and Ed25519 sidecars are generated only after Authenticode.

Only then add the protected Windows signing job, set `publisher_subject` to the
certificate's exact subject, and change `enforcement_state` to `active` in the
same reviewed PR. Public RC and stable releases must not fall back to unsigned
output after activation.

## Revocation and incident response

- Preserve Artifact Signing audit logs and the GitHub run URL as release evidence.
- Revoke a suspect signing certificate immediately and stop the release.
- Record affected SHA-256 values, tags, signing times, and the revocation reason.
- Rehearse a replacement profile without publishing before activation.
