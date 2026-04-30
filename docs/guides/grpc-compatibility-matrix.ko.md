[English](./grpc-compatibility-matrix.md) | [한국어](./grpc-compatibility-matrix.ko.md)

# gRPC 호환성 매트릭스 (v1)

이 문서는 ONESHIM gRPC v1 계약의 호환성 정책을 정의합니다.

## 적용 범위

- 계약 원천(릴리즈 플로우에서 제공되는 경우)
- 생성 바인딩: `crates/oneshim-network/src/proto/generated/*`
- 런타임 클라이언트: `crates/oneshim-network/src/grpc/*`

## 매트릭스

| 변경 유형 | Backward 호환성 | Forward 호환성 | v1 허용 여부 |
|---|---|---|---|
| 새 optional 필드 추가(새 tag 사용) | Yes | Yes (구버전 클라이언트가 무시) | Yes |
| enum 값 추가 | Yes (unknown-safe 처리 전제) | Partial | Yes |
| 새 RPC 메서드 추가 | Yes | Yes | Yes |
| message/enum/RPC 심볼 이름 변경 | No | No | No |
| 필드 tag 제거 | No | No | No |
| 기존 tag 재사용(의미 변경) | No | No | No |
| RPC 메서드 제거 | No | No | No |
| 필드 wire type 변경 | No | No | No |

## CI 강제 정책

- 스크립트: `scripts/verify-grpc-compatibility.sh`
- 워크플로: `.github/workflows/grpc-governance.yml`

호환성 게이트는 생성 계약 파일에서 잠재적 breaking removal을 차단합니다:

- `pub struct` / `pub enum` / `pub trait` / `pub mod` 제거
- 생성 gRPC client의 `pub async fn` 제거
- `#[prost(... tag = "N")]` 라인 제거

## 로컬 검증

```bash
./scripts/verify-grpc-compatibility.sh
./scripts/verify-grpc-readiness.sh
```

## 예외 처리

의도된 major 마이그레이션의 경우, 로컬 게이트를 1회 우회할 수 있습니다:

```bash
GRPC_COMPAT_ALLOW_BREAKING=1 ./scripts/verify-grpc-compatibility.sh
```

이 우회는 정책 면제가 아니며, 마이그레이션 계획과 릴리즈 노트는 필수입니다.
