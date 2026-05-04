# Integrity Runbook

This runbook describes how to operate Maekon integrity controls in day-to-day development and release workflows.

## 1. Pre-merge (PR) Checklist

Run locally before opening or updating a PR:

```bash
cargo check --workspace
cargo test -p oneshim-web
./scripts/verify-integrity.sh
```

Expected output:

- Integrity policy tests pass (`oneshim-core`)
- Signature verification tests pass (`oneshim-app`)
- Supply-chain gates pass (`audit`, `deny`, `vet`)
- SBOM files generated under `artifacts/integrity/*.sbom.cdx.json`, including `oneshim-app.sbom.cdx.json` for the Tauri composition root.

## 2. CI Gates

The following workflows enforce integrity in CI:

- `CI` (`.github/workflows/ci.yml`): lint + tests for the fast PR lane
- `Security & Compliance` (`.github/workflows/security-compliance.yml`): supply-chain checks + SBOM on `main`/`develop`, schedule, or manual dispatch
- `Integrity Gates` (`.github/workflows/integrity-gates.yml`): policy + signature + supply-chain checks on `main`/`develop`, schedule, or manual dispatch
- `Release Smoke` (`.github/workflows/release-smoke.yml`): cross-platform desktop release smoke on `main`/`develop` or manual dispatch

PRs must not bypass the fast-lane workflows. Full integrity, supply-chain, and release smoke validation continues after merge, on schedule, and before release promotion.

Current documented exception:

- `RUSTSEC-2024-0429` (`glib 0.18.5`) is ignored in `cargo audit` and `cargo deny`.
- Scope is limited to the Linux GTK3 / `webkit2gtk` / `wry` / Tauri transitive chain.
- Official public Linux artifacts (`.deb`, Linux tarball) remain deferred while this runtime exception is active. Linux source builds and internal release rehearsals may continue, but public release notes and install docs must not advertise Linux downloads.
- Exit criterion: remove the exception once the upstream desktop stack moves to patched `gtk-rs-core` / `glib` releases, then restore the Linux release artifact lane.
- `RUSTSEC-2026-0097` (`rand 0.7.x`) is ignored only as a low-risk build-time transitive advisory. Before every public release candidate, confirm it remains absent from the normal runtime dependency graph:

  ```bash
  cargo tree --locked --edges normal -i rand@0.7.3
  ```

- Treat any new advisory outside these documented exceptions as a release blocker. If a documented build-time exception becomes a runtime path, treat it as a release blocker too.

## 3. Release Procedure

Release workflow (`.github/workflows/release.yml`) performs:

1. Artifact build (platform matrix)
2. SHA-256 sidecar generation (`.sha256`)
3. Ed25519 signing (`.sig`)
4. Provenance attestation for release artifacts
5. GitHub release publishing

Release artifacts are considered valid only when checksum + signature + provenance are all present.

Current public artifact scope:

- macOS and Windows artifacts are official public release outputs.
- Linux artifacts are not official public release outputs while the `glib 0.18.x` runtime advisory exception remains active.

## 4. Key Management Basics

- Store update signing private key only in GitHub Actions secrets.
- Never commit private key material.
- Keep public key in client config default and update policy checks.
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
