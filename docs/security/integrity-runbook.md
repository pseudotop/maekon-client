# Integrity Runbook

This runbook describes how to operate Maekon integrity controls in day-to-day development and release workflows.

## 1. Pre-merge (PR) Checklist

Run locally before opening or updating a PR:

```bash
cargo check --workspace
cargo test -p maekon-web
./scripts/verify-integrity.sh
```

Expected output:

- Integrity policy tests pass (`maekon-core`)
- Signature verification tests pass (`maekon-app`)
- Supply-chain gates pass (`audit`, `deny`, `vet`)
- SBOM generated at `artifacts/integrity/sbom.cdx.json`

## 2. CI Gates

The following workflows enforce integrity in CI:

- `CI` (`.github/workflows/ci.yml`): lint + tests for the fast PR lane
- `Security & Compliance` (`.github/workflows/security-compliance.yml`): supply-chain checks + SBOM on pull requests, pushes to `main`, or manual dispatch
- `Integrity Gates` (`.github/workflows/integrity-gates.yml`): manual-only standalone policy + signature + supply-chain checks. Run explicitly before release promotion.
- `Release Smoke` (`.github/workflows/release-smoke.yml`): manual-only cross-platform desktop release smoke. Run explicitly before RC/stable promotion when release evidence is required.

PRs must not bypass the fast-lane workflows. Full integrity and release-smoke validation must be produced by explicit manual dispatch before release promotion; `security-compliance.yml` contributes PR/main supply-chain evidence but is not a release-promotion substitute.

Current trigger boundary:

- Public PR blocking evidence comes from `ci.yml` plus parent/private required checks.
- `integrity-gates.yml` and `release-smoke.yml` do not run automatically today; release operators must dispatch them and attach the resulting run URLs or artifacts to the release-decision manifest.
- `security-compliance.yml` runs on PRs, pushes to `main`, and manual dispatch; do not treat a green fast PR lane as a substitute for release-promotion evidence.

Current documented exception:

- No RustSec advisory is ignored in the local integrity script.
- The previous `RUSTSEC-2024-0429` (`glib 0.18.5`) exception was retired by the E31 GTK4/WebKitGTK 6 migration.
- `cargo audit` must stay at zero vulnerabilities. Existing informational
  transitive warnings are tracked in `deny.toml` and issue #5431; release
  operators should treat a new warning ID or a changed dependency path as a
  fresh triage item.
- Treat any new advisory as a release blocker unless it has an explicit repository policy entry with owner, expiry, and issue reference.
- GitHub release gating is stricter than the local `cargo audit` exception list:
  any open Dependabot or CodeQL alert must be fixed or explicitly recorded in
  `supply-chain/release-alert-acceptance.json` with an expiry and review issue.

## 3. Release Procedure

Release workflow (`.github/workflows/release.yml`) performs:

1. Artifact build (platform matrix)
2. SHA-256 sidecar generation (`.sha256`)
3. Ed25519 signing (`.sig`)
4. Provenance attestation for release artifacts
5. GitHub release publishing

Release artifacts are considered valid only when checksum + signature + provenance are all present.

## 4. Key Management Basics

- Store update signing private key only in GitHub Actions secrets.
- Never commit private key material.
- Keep public release keys in the built-in updater trusted-key array and align install-script defaults.
- For key rotation:
  - Publish new public key in a release that still validates with the old key path.
  - Rotate private key in CI secret.
  - Document effective date and rollback plan.

### Local Rehearsal

```bash
./scripts/rehearse-key-rotation.sh
```

Use generated artifacts in `artifacts/integrity/key-rotation/` to verify both old/new signatures before production cutover.

## 4.1 Signed Policy Bundle Startup Gate

When using signed runtime policy bundles, set in config:

```json
{
  "update": {
    "min_allowed_version": "0.0.1"
  },
  "integrity": {
    "enabled": true,
    "require_signed_policy_bundle": true,
    "policy_file_path": "./runtime-policy.json",
    "policy_signature_path": "./runtime-policy.json.sig",
    "policy_public_key": "<base64-ed25519-public-key>"
  }
}
```

`update.min_allowed_version` defaults to the current build version when omitted;
keep it explicit in managed bundles when the fleet baseline must be raised above
the shipped default.

Startup will fail closed if bundle verification fails.

## 5. Incident Handling

If any integrity gate fails in CI:

1. Treat as release blocker.
2. Identify failing layer (policy / signature / supply chain / SBOM / provenance).
3. Fix root cause and rerun full integrity script.
4. Record impact and remediation in PR notes.

For vulnerability disclosure and response timelines, follow `SECURITY.md`.

## 6. Future Integration Constraints

Even in standalone mode, keep these ready for future server/third-party integrations:

- Signed envelope fields in transport contracts (`nonce`, `timestamp`, `key_id`, `sig`)
- Replay-safe semantics
- Capability-scoped third-party access model
- Fail-closed default for any trust decision

## 7. gRPC mTLS Certificate Operations

For mTLS-enabled environments, manage transport keys separately from update-signing keys.

### 7.1 Required Config Fields

Set these in `config.json` when `grpc.use_tls=true`:

- `grpc.tls_domain_name`
- `grpc.tls_ca_cert_path`
- `grpc.tls_client_cert_path` (required if `grpc.mtls_enabled=true`)
- `grpc.tls_client_key_path` (required if `grpc.mtls_enabled=true`)

### 7.2 Operational Policy

- Keep client private keys out of the repository and out of release artifacts.
- Rotate client certificates on a fixed schedule (for example, every 90 days).
- Validate mTLS policy in CI with:

```bash
./scripts/verify-grpc-mtls-config.sh
```

### 7.3 Rotation Drill (Staging)

1. Issue a new client certificate/key pair from the staging CA.
2. Update `tls_client_cert_path` and `tls_client_key_path` in staging config.
3. Run `./scripts/verify-grpc-readiness.sh` and confirm all checks are green.
4. Perform a controlled rollout in production with rollback path documented.
