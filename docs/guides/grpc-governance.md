[English](./grpc-governance.md) | [한국어](./grpc-governance.ko.md)

# gRPC Governance Guide

This guide defines the minimum governance baseline for MAEKON gRPC client operations.

## Scope

- Consumer client contracts: `api/proto/oneshim/client/v1/*`
- Dashboard service contract: `api/proto/oneshim/dashboard/v1/*`
- Consumer generated code: `crates/maekon-network/src/proto/generated/*`
- Dashboard generated code: `crates/maekon-web/src/proto/generated/oneshim.dashboard.v1.rs`
- gRPC runtime code: `crates/maekon-network/src/grpc/*`, `crates/maekon-web/src/grpc/*`

## Baseline Rules

1. **Contract integrity**
   - Proto files under `api/proto` are the single source of truth.
   - Generated Rust files in `crates/maekon-network/src/proto/generated` and `crates/maekon-web/src/proto/generated` must stay committed and up-to-date.
2. **Feature-gated safety**
   - All gRPC changes must pass compile/test with `--features grpc`.
   - `maekon-app` wiring must compile with `--features grpc`.
3. **Fallback guarantee**
   - `GrpcConfig` fallback endpoints must be preserved and tested.
   - Protocol selection behavior (`gRPC` vs `REST`) must remain deterministic.
4. **Operational visibility**
   - gRPC gate failures are release blockers for gRPC-enabled builds.

## CI Gate

- Workflow: `.github/workflows/grpc-governance.yml`
- Compatibility script: `scripts/verify-grpc-compatibility.sh`
- mTLS validation script: `scripts/verify-grpc-mtls-config.sh`
- Script: `scripts/verify-grpc-readiness.sh`
- Policy matrix: `docs/guides/grpc-compatibility-matrix.md`

The script enforces:

```bash
./scripts/verify-grpc-compatibility.sh
./scripts/verify-grpc-mtls-config.sh
cargo check -p maekon-network --features grpc
cargo test -p maekon-network --features grpc
cargo test -p maekon-network --features grpc reconnect_
cargo test -p maekon-network --features grpc chaos_
cargo test -p maekon-network --features grpc proxy_fault_
cargo check -p maekon-app --features maekon-network/grpc
git diff --quiet -- crates/maekon-network/src/proto/generated
```

## Release Safety Checklist

- Proto changes reviewed for backward compatibility impact.
- Compatibility matrix reviewed and aligned (`docs/guides/grpc-compatibility-matrix.md`).
- mTLS configuration policy checks green (`scripts/verify-grpc-mtls-config.sh`).
- Generated files refreshed and committed.
- gRPC governance workflow green.
- gRPC error mapping guide reviewed (`docs/guides/grpc-error-mapping.md`).
- Integrity workflows green (`integrity-gates`, `security-compliance`).

## Next Hardening Steps

1. Add end-to-end gRPC chaos tests with an external fault proxy container in CI.
