[English](./grpc-compatibility-matrix.md) | [한국어](./grpc-compatibility-matrix.ko.md)

# gRPC Compatibility Matrix (v1)

This document defines the compatibility policy for ONESHIM gRPC contracts in v1.

## Scope

- Contract source of truth (when available in release flow)
- Generated bindings in `crates/oneshim-network/src/proto/generated/*`
- Runtime clients in `crates/oneshim-network/src/grpc/*`

## Matrix

| Change Type | Backward Compatibility | Forward Compatibility | Allowed in v1 |
|---|---|---|---|
| Add new optional field with new tag | Yes | Yes (ignored by old clients) | Yes |
| Add new enum value | Yes (if unknown-safe handling) | Partial | Yes |
| Add new RPC method | Yes | Yes | Yes |
| Rename message/enum/RPC symbol | No | No | No |
| Remove field tag | No | No | No |
| Reuse existing field tag for new meaning | No | No | No |
| Remove RPC method | No | No | No |
| Change field wire type | No | No | No |

## CI Enforcement

- Script: `scripts/verify-grpc-compatibility.sh`
- Workflow: `.github/workflows/grpc-governance.yml`

The compatibility gate blocks potentially breaking removals from generated contract files:

- removed `pub struct` / `pub enum` / `pub trait` / `pub mod`
- removed `pub async fn` in generated gRPC clients
- removed `#[prost(... tag = "N")]` lines

## Local Verification

```bash
./scripts/verify-grpc-compatibility.sh
./scripts/verify-grpc-readiness.sh
```

## Exceptional Cases

For intentional major migrations, you may bypass the local gate once with:

```bash
GRPC_COMPAT_ALLOW_BREAKING=1 ./scripts/verify-grpc-compatibility.sh
```

This bypass is not a policy override. A migration plan and release note are still required.
