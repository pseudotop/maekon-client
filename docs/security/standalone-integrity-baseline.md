# Standalone Integrity Baseline

This document defines the mandatory integrity baseline for Maekon in standalone mode.

The goal is to keep the standalone trust model strict today, while making future server and third-party integrations additive rather than disruptive.

## Security Objectives

- Fail closed when integrity checks fail.
- Keep update/install trust anchored in cryptographic verification.
- Produce machine-verifiable supply-chain evidence for each release.
- Keep boundary contracts stable for future remote integrations.

## Mandatory Controls

### 1) Update Integrity

- `update.require_signature_verification` MUST stay enabled when updates are enabled.
- `update.signature_public_key` MUST be a valid Ed25519 public key (32-byte decoded payload).
- Update artifacts MUST pass both SHA-256 and Ed25519 verification before extraction/install.
- Signature/checksum sidecars (`.sig`, `.sha256`) MUST be generated and published with every release artifact.
- `update.min_allowed_version` SHOULD be set to enforce anti-rollback version floors.

### 1.1) Signed Policy Bundle (Startup Gate)

- When `integrity.require_signed_policy_bundle=true`, startup MUST verify:
  - `integrity.policy_file_path`
  - `integrity.policy_signature_path`
  - `integrity.policy_public_key` (authoritative — configure this explicitly).
    Historical note: an `update.signature_public_key` fallback path still exists
    in the integrity guard, but since the D9/I-4 cleanup `update.signature_public_key`
    defaults to an empty string, so the fallback is inert unless that field is
    explicitly configured. Prefer `integrity.policy_public_key`.
- If signature validation fails, the app MUST fail closed at startup.
- Release pipeline SHOULD publish `runtime-policy.json`, `.sha256`, and `.sig` together.

### 2) Supply-Chain Integrity

- RustSec scan: `cargo audit`
- Dependency policy: `cargo deny check licenses advisories sources bans`
- Vet policy: `cargo vet check` (blocking on post-merge, scheduled, and manually dispatched integrity workflows)
- SBOM: `cargo cyclonedx --workspace`
- Provenance attestation: GitHub artifact attestation on release artifacts

Current exception policy:

- No RustSec advisory is ignored by the local integrity script.
- The previous `RUSTSEC-2024-0429` (`glib 0.18.5`) exception was retired by the E31 GTK4/WebKitGTK 6 migration.
- `cargo audit` must report zero vulnerabilities. Informational transitive
  warnings are tracked in repository policy and issue #5431 until upstream
  Tauri/urlpattern/tokenizers releases remove them.
- Any new advisory must be fixed, or explicitly triaged in repository policy with an owner, expiry, and issue reference.

### 3) Runtime Boundary Rules

- Web handlers MUST not access SQLite internals directly (`conn_ref` forbidden in handlers).
- Data access for web handlers MUST pass through storage adapter APIs.
- Integrity-sensitive behavior MUST fail closed (startup/check/update stage), not warn-and-continue.

### 4) Documentation and Auditability

- Integrity policy changes MUST update this baseline and `docs/security/integrity-runbook.md`.
- Security process and disclosure policy remain in `SECURITY.md`.

## Local Verification Command

```bash
./scripts/verify-integrity.sh
```

This command verifies integrity policy tests, signature verification tests, supply-chain checks, and SBOM generation.

## Key Rotation Rehearsal

```bash
./scripts/rehearse-key-rotation.sh
```

This script generates local rehearsal artifacts (old/new key + signatures) under
`artifacts/integrity/key-rotation/` so operators can validate dual-signature migration flow.

## Future Integration Readiness (Server / Third-Party)

The following are not required for standalone runtime now, but are required design constraints from this phase onward:

- Signed request envelope fields reserved in contracts: `nonce`, `timestamp`, `key_id`, `sig`
- Replay-protection-ready protocol semantics
- Capability-scoped third-party integration contracts (least privilege by default)
- Root/online key separation and documented key-rotation process
