[English](./grpc-error-mapping.md) | [한국어](./grpc-error-mapping.ko.md)

# gRPC Error Mapping Guide

This guide defines how the Maekon client maps gRPC status errors into `NetworkError` (which then converts to `CoreError` at the port boundary via `impl From<NetworkError> for CoreError`).

## Source

- Status → NetworkError: `crates/oneshim-network/src/grpc/error_mapping.rs`
- NetworkError → CoreError: `crates/oneshim-network/src/error.rs` `impl From<NetworkError> for CoreError`
- Consumers:
  - `crates/oneshim-network/src/grpc/auth_client.rs`
  - `crates/oneshim-network/src/grpc/session_client.rs`
  - `crates/oneshim-network/src/grpc/context_client.rs`
  - `crates/oneshim-network/src/grpc/health_client.rs`

## Mapping Policy

Two-step mapping — gRPC Status → `NetworkError` → `CoreError`:

| gRPC `Code` | `NetworkError` | Final `CoreError` + wire code (per [ADR-019](../architecture/ADR-019-error-code-infrastructure.md)) | Notes |
|---|---|---|---|
| `Unauthenticated`, `PermissionDenied` | `Auth` | `CoreError::Auth { code: AuthCode::Failed, .. }` → `auth.failed` | Authentication/authorization failure |
| `NotFound` | `NotFound { resource_type, id }` | `CoreError::NotFound { code: NotFoundCode::ResourceMissing, .. }` → `not_found.resource_missing` | Operation name + status message propagated |
| `InvalidArgument`, `FailedPrecondition`, `OutOfRange` | `Validation { field, message }` | `CoreError::Validation { code: ValidationCode::InvalidField, .. }` → `validation.invalid_field` | Reported as request validation failure |
| `ResourceExhausted` | `RateLimited { retry_after_secs }` | `CoreError::RateLimit { code: NetworkCode::RateLimit, .. }` → `network.rate_limit` | Uses `retry-after` or `x-retry-after-seconds`, default `60` |
| `Unavailable` | `ServiceUnavailable` | `CoreError::ServiceUnavailable { code: ServiceCode::Unavailable, .. }` → `service.unavailable` | Service availability outage |
| `DeadlineExceeded` | `Timeout { timeout_ms: 0 }` | `CoreError::RequestTimeout { code: NetworkCode::Timeout, .. }` → `network.timeout` | Client-side deadline elapsed (sentinel timeout_ms=0; request-site logs the real timeout) |
| `Unimplemented` | `NotFound { resource_type: "grpc_method:OP", id }` | `CoreError::NotFound { code: NotFoundCode::ResourceMissing, .. }` → `not_found.resource_missing` | Server doesn't implement the RPC — typically a client/server version skew. Non-retryable |
| `Internal`, `DataLoss` | `Internal` | `CoreError::Internal { code: InternalCode::Generic, .. }` → `internal.generic` | Server-reported internal failure (Iter-92). DataLoss is catastrophic — alert on frequency |
| `AlreadyExists` | `Validation { field: "grpc_request", message }` | `CoreError::Validation { code: ValidationCode::InvalidField, .. }` → `validation.invalid_field` | Client-side conflict (Iter-92). Non-retryable; caller must use a different key |
| `Aborted` | `ServiceUnavailable` | `CoreError::ServiceUnavailable { code: ServiceCode::Unavailable, .. }` → `service.unavailable` | Transient concurrency conflict (Iter-92). Retryable with backoff |
| `Cancelled`, `Unknown` | `Http` | `CoreError::Network { code: NetworkCode::Generic, .. }` → `network.generic` | Remaining wildcard: truly unclassified or client-side cancellation |

## Consuming the wire code

Per [ADR-019](../architecture/ADR-019-error-code-infrastructure.md), every `CoreError` variant carries a typed `code: XxxCode` field. Use `err.code() -> &'static str` to obtain the stable wire-format string for Grafana/logs/i18n:

```rust
let err: CoreError = map_grpc_status_error("login", status).into();
tracing::error!(code = err.code(), %err, "grpc call failed");
// → logs include `code="auth.failed"` etc.
```

Released code strings are immutable (wire contract). Additions require updating `crates/oneshim-core/tests/wire_contract_snapshot.expected.txt`.

## Verification

```bash
cargo test -p oneshim-network --features grpc error_mapping
./scripts/verify-grpc-readiness.sh
```
