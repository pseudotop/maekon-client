# AI / Integration Readiness Checklist

Use this checklist before merging changes that affect:

- AI provider surfaces
- AI settings and credential flows
- OCR / multimodal routing
- Integration session, egress, inbox, auth, or telemetry
- External integration contracts or compatibility harnesses

If any required item is not green, do not treat the area as release-ready.

## 1. Architecture and Boundary Gates

- [ ] `oneshim-core` remains the contract layer. No delivery/runtime logic was pushed into domain crates.
- [ ] Adapter crates still implement `oneshim-core` ports instead of introducing direct adapter-to-adapter coupling.
- [ ] `oneshim-app` remains the composition root and runtime orchestrator.
- [ ] `oneshim-web` remains a delivery layer. Handlers stay thin and service/context boundaries remain explicit.
- [ ] External egress still passes policy, privacy, consent, and audit paths.
- [ ] ADRs remain aligned with the implementation baseline:
  - `docs/architecture/ADR-001-rust-client-architecture-patterns.md`
  - `docs/architecture/ADR-008-network-resilience-patterns.md`
  - `docs/architecture/ADR-009-client-architecture-baseline.md`
  - `docs/architecture/ADR-010-local-integration-harness-boundary.md`

## 2. AI Provider Surface Gates

- [ ] Provider surface catalog is still the canonical source of truth for surface metadata.
- [ ] Settings save/load keeps credential bindings, access mode, and surface selection consistent.
- [ ] No legacy AI access path or legacy credential backend was reintroduced.
- [ ] Direct HTTP, subprocess CLI, self-hosted, and managed OAuth paths still resolve through the current surface model.
- [ ] OCR and multimodal paths still enforce model/surface compatibility rules.
- [ ] Self-hosted readiness and model discovery behavior remain consistent with current surface capability semantics.

## 3. Integration Runtime Gates

- [ ] Bootstrap auth, session management, egress, inbox, and telemetry still follow the `integration` ports and adapters model.
- [ ] Transport resilience behavior still covers backoff, retry, and reconnect semantics.
- [ ] Auth lifecycle still supports the current device/OIDC + proof flow semantics used by the client.
- [ ] Privacy and retention behavior still applies to outbound insight payloads and stored prompt state.
- [ ] Integration status/telemetry read models still expose enough information to debug failures without leaking secrets.

## 4. Fake Server Compatibility Gates

- [ ] The local fake server harness still passes all client-side compatibility scenarios.
- [ ] HTTP transport scenarios still pass:
  - bootstrap / heartbeat / disconnect
  - egress / inbox roundtrip
  - partial ack
  - `Retry-After` recovery
  - prompt receipt roundtrip
- [ ] WebSocket transport scenarios still pass:
  - outbound ack
  - prompt signal and prompt batch delivery
  - malformed payload recovery
  - unsupported prompt-event recovery
  - live channel reconnect and repeated churn
  - multi-connection handling
  - large prompt batches and large outbound batches
  - DPoP handshake/header propagation

## 5. Required Verification Commands

- [ ] `cargo test -p oneshim-network --test integration_fake_server -- --nocapture`
- [ ] `cargo test -p oneshim-web integration --lib -- --nocapture`
- [ ] `cargo test -p oneshim-web settings_service --lib -- --nocapture`
- [ ] `cargo test -p oneshim-web ai_model_catalog_service --lib -- --nocapture`
- [ ] `cargo test -p oneshim-app --features server build_integration_runtime -- --nocapture`
- [ ] `cargo check --workspace`
- [ ] `cargo clippy -p oneshim-network --all-targets -- -D warnings`
- [ ] `cargo clippy -p oneshim-web --all-targets -- -D warnings`
- [ ] `cargo clippy -p oneshim-app --all-targets -- -D warnings`
- [ ] `cargo fmt --all --check`
- [ ] `git diff --check`

## 6. Final Human Review

- [ ] The change does not widen external exposure beyond the current architecture baseline.
- [ ] The change does not bypass policy/privacy/audit for outbound integration traffic.
- [ ] The change does not add ad-hoc handler logic that should live in a service, assembler, or context.
- [ ] The change does not create a second way to do the same AI/integration flow.
- [ ] The change is understandable enough that a follow-up developer can locate the relevant context, service, and adapter without code archaeology.
