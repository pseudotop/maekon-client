[English](./grpc-governance.md) | [한국어](./grpc-governance.ko.md)

# gRPC Governance Guide

This guide defines the minimum governance baseline for ONESHIM gRPC client operations.

## Scope

- `crates/oneshim-network/src/grpc/*`
- `crates/oneshim-network/src/proto/generated/*`
- `api/proto/oneshim/v1/*`

## Baseline Rules

1. **Contract integrity**
   - Proto files under `api/proto` are the single source of truth.
   - Generated Rust files in `crates/oneshim-network/src/proto/generated` must stay committed and up-to-date.
2. **Feature-gated safety**
   - All gRPC changes must pass compile/test with `--features grpc`.
   - `oneshim-app` wiring must compile with `--features grpc`.
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
cargo check -p oneshim-network --features grpc
cargo test -p oneshim-network --features grpc
cargo test -p oneshim-network --features grpc reconnect_
cargo test -p oneshim-network --features grpc chaos_
cargo test -p oneshim-network --features grpc proxy_fault_
cargo check -p oneshim-app --features oneshim-network/grpc
git diff --quiet -- crates/oneshim-network/src/proto/generated
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
