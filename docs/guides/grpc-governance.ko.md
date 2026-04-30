[English](./grpc-governance.md) | [한국어](./grpc-governance.ko.md)

# gRPC 거버넌스 가이드

이 문서는 ONESHIM gRPC 클라이언트 운영을 위한 최소 거버넌스 기준을 정의합니다.

## 적용 범위

- `crates/oneshim-network/src/grpc/*`
- `crates/oneshim-network/src/proto/generated/*`
- `api/proto/oneshim/v1/*`

## 기본 규칙

1. **계약 무결성**
   - `api/proto` 하위 proto 파일을 단일 진실 원천(SSOT)으로 유지합니다.
   - `crates/oneshim-network/src/proto/generated` 생성 코드는 항상 최신 상태로 커밋되어야 합니다.
2. **Feature-gated 안정성**
   - 모든 gRPC 변경은 `--features grpc` 빌드/테스트를 통과해야 합니다.
   - `oneshim-app` 배선도 `--features grpc`로 컴파일 검증해야 합니다.
3. **Fallback 보장**
   - `GrpcConfig` fallback endpoint 규칙을 유지하고 테스트해야 합니다.
   - `gRPC`/`REST` 선택 동작은 결정적으로 유지되어야 합니다.
4. **운영 가시성**
   - gRPC 게이트 실패는 gRPC 빌드 기준 릴리즈 차단 항목입니다.

## CI 게이트

- 워크플로: `.github/workflows/grpc-governance.yml`
- 호환성 스크립트: `scripts/verify-grpc-compatibility.sh`
- mTLS 검증 스크립트: `scripts/verify-grpc-mtls-config.sh`
- 스크립트: `scripts/verify-grpc-readiness.sh`
- 정책 매트릭스: `docs/guides/grpc-compatibility-matrix.md`

스크립트 강제 항목:

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

## 릴리즈 안전 체크리스트

- Proto 변경의 호환성 영향 검토 완료
- 호환성 매트릭스 검토/정합성 확인 완료 (`docs/guides/grpc-compatibility-matrix.md`)
- mTLS 설정 정책 검증 green (`scripts/verify-grpc-mtls-config.sh`)
- 생성 코드 재생성/커밋 완료
- gRPC 거버넌스 워크플로 green
- gRPC 에러 매핑 가이드 검토 완료 (`docs/guides/grpc-error-mapping.md`)
- 무결성 워크플로(`integrity-gates`, `security-compliance`) green

## 다음 강화 항목

1. CI에서 external fault proxy 컨테이너를 이용한 end-to-end gRPC chaos 테스트 추가
