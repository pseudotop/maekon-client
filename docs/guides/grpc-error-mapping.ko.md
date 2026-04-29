[English](./grpc-error-mapping.md) | [한국어](./grpc-error-mapping.ko.md)

# gRPC 에러 매핑 가이드

이 문서는 Maekon 클라이언트가 gRPC status 에러를 `NetworkError`로 변환하는 기준을 정의합니다. `NetworkError`는 포트 경계에서 `impl From<NetworkError> for CoreError`를 통해 `CoreError`로 변환됩니다.

## 소스

- Status → NetworkError: `crates/oneshim-network/src/grpc/error_mapping.rs`
- NetworkError → CoreError: `crates/oneshim-network/src/error.rs` `impl From<NetworkError> for CoreError`
- 사용처:
  - `crates/oneshim-network/src/grpc/auth_client.rs`
  - `crates/oneshim-network/src/grpc/session_client.rs`
  - `crates/oneshim-network/src/grpc/context_client.rs`
  - `crates/oneshim-network/src/grpc/health_client.rs`

## 매핑 정책

2단계 매핑 — gRPC Status → `NetworkError` → `CoreError`:

| gRPC `Code` | `NetworkError` | 최종 `CoreError` + wire code ([ADR-019](../architecture/ADR-019-error-code-infrastructure.ko.md) 기준) | 설명 |
|---|---|---|---|
| `Unauthenticated`, `PermissionDenied` | `Auth` | `CoreError::Auth { code: AuthCode::Failed, .. }` → `auth.failed` | 인증/인가 실패 |
| `NotFound` | `NotFound { resource_type, id }` | `CoreError::NotFound { code: NotFoundCode::ResourceMissing, .. }` → `not_found.resource_missing` | operation 이름 + status message 전달 |
| `InvalidArgument`, `FailedPrecondition`, `OutOfRange` | `Validation { field, message }` | `CoreError::Validation { code: ValidationCode::InvalidField, .. }` → `validation.invalid_field` | 요청 유효성 실패 |
| `ResourceExhausted` | `RateLimited { retry_after_secs }` | `CoreError::RateLimit { code: NetworkCode::RateLimit, .. }` → `network.rate_limit` | `retry-after` 또는 `x-retry-after-seconds` 사용, 기본값 `60` |
| `Unavailable` | `ServiceUnavailable` | `CoreError::ServiceUnavailable { code: ServiceCode::Unavailable, .. }` → `service.unavailable` | 서비스 가용성 장애 |
| `DeadlineExceeded` | `Timeout { timeout_ms: 0 }` | `CoreError::RequestTimeout { code: NetworkCode::Timeout, .. }` → `network.timeout` | 클라이언트측 데드라인 초과 (sentinel timeout_ms=0; 실제 timeout은 request-site 로그 참조) |
| `Unimplemented` | `NotFound { resource_type: "grpc_method:OP", id }` | `CoreError::NotFound { code: NotFoundCode::ResourceMissing, .. }` → `not_found.resource_missing` | 서버가 해당 RPC 미구현 — 일반적으로 client/server 버전 불일치. 재시도 불가 |
| `Internal`, `DataLoss` | `Internal` | `CoreError::Internal { code: InternalCode::Generic, .. }` → `internal.generic` | 서버측 내부 실패 (Iter-92). DataLoss는 catastrophic — 빈도에 따라 알람 |
| `AlreadyExists` | `Validation { field: "grpc_request", message }` | `CoreError::Validation { code: ValidationCode::InvalidField, .. }` → `validation.invalid_field` | 클라이언트측 충돌 (Iter-92). 재시도 불가; 다른 키 사용 필요 |
| `Aborted` | `ServiceUnavailable` | `CoreError::ServiceUnavailable { code: ServiceCode::Unavailable, .. }` → `service.unavailable` | 일시적 동시성 충돌 (Iter-92). backoff 재시도 가능 |
| `Cancelled`, `Unknown` | `Http` | `CoreError::Network { code: NetworkCode::Generic, .. }` → `network.generic` | 잔여 wildcard: 진짜 미분류 또는 클라이언트측 취소 |

## Wire 코드 소비

[ADR-019](../architecture/ADR-019-error-code-infrastructure.ko.md)에 따라 모든 `CoreError` variant는 타입화된 `code: XxxCode` 필드를 보유합니다. `err.code() -> &'static str`로 Grafana/로그/i18n용 안정 wire-format 문자열을 획득:

```rust
let err: CoreError = map_grpc_status_error("login", status).into();
tracing::error!(code = err.code(), %err, "grpc call failed");
// → 로그에 `code="auth.failed"` 등 포함
```

릴리스된 code 문자열은 불변 (wire contract). 추가 시 `crates/oneshim-core/tests/wire_contract_snapshot.expected.txt` 업데이트 필요.

## 검증

```bash
cargo test -p oneshim-network --features grpc error_mapping
./scripts/verify-grpc-readiness.sh
```
