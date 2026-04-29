[English](./standalone-integrity-baseline.md) | [한국어](./standalone-integrity-baseline.ko.md)

# Standalone 무결성 베이스라인

이 문서는 Maekon 의 standalone 모드에서 반드시 지켜야 하는 무결성 기준을 정의합니다.

목표는 현재 standalone 신뢰 모델을 엄격하게 유지하면서, 추후 서버/서드파티 연동 시 아키텍처를 재설계하지 않고 확장 가능하게 만드는 것입니다.

## 보안 목표

- 무결성 검증 실패 시 fail-closed 동작
- 업데이트/설치 신뢰 체인을 암호학 검증으로 고정
- 릴리즈 단위 공급망 증거를 기계적으로 검증 가능하게 유지
- 미래 원격 연동을 위한 계약 경계의 안정성 확보

## 필수 통제 항목

### 1) 업데이트 무결성

- 업데이트가 활성화된 경우 `update.require_signature_verification`은 반드시 true여야 합니다.
- `update.signature_public_key`는 유효한 Ed25519 공개키(디코딩 후 32바이트)여야 합니다.
- 아티팩트는 설치 전에 SHA-256 + Ed25519 검증을 모두 통과해야 합니다.
- 모든 릴리즈 아티팩트에 `.sig`, `.sha256`를 함께 생성/배포해야 합니다.
- 롤백 공격 방지를 위해 `update.min_allowed_version`을 설정하는 것을 권장합니다.

### 1.1) 서명된 정책 번들 (시작 게이트)

- `integrity.require_signed_policy_bundle=true`일 때 시작 시 다음 항목을 반드시 검증합니다.
  - `integrity.policy_file_path`
  - `integrity.policy_signature_path`
  - `integrity.policy_public_key` (권장 설정 — 이 값을 명시적으로 설정해야 합니다).
    과거에는 `update.signature_public_key`가 fallback 경로로 사용되었지만,
    D9/I-4 정리 이후 `update.signature_public_key` 기본값이 빈 문자열이므로,
    해당 필드를 명시적으로 설정하지 않는 이상 fallback 경로는 작동하지 않습니다.
    `integrity.policy_public_key`를 직접 설정하는 것을 권장합니다.
- 서명 검증 실패 시 앱은 시작 단계에서 fail-closed로 종료되어야 합니다.
- 릴리즈 파이프라인은 `runtime-policy.json`, `.sha256`, `.sig`를 함께 배포해야 합니다.

### 2) 공급망 무결성

- RustSec: `cargo audit`
- 의존성 정책: `cargo deny check licenses advisories sources bans`
- vet 정책: `cargo vet check` (post-merge, schedule, manual dispatch 무결성 워크플로에서는 blocking)
- SBOM: `cargo cyclonedx --workspace`
- Provenance: 릴리즈 아티팩트 Attestation 생성

문서화된 예외:

- `RUSTSEC-2024-0429` (`glib 0.18.5`)는 Linux 전용 GTK3 / `webkit2gtk` / `wry` / Tauri 전이 의존성이라 현재 프로젝트가 직접 해소할 수 없어 한시적으로 허용합니다.
- 이 예외는 저장소 정책(`deny.toml`, integrity workflow/script)에 명시적으로 남아 있어야 하며, upstream desktop stack이 패치된 `glib` / `gtk-rs-core`로 이동하면 즉시 제거해야 합니다.

### 3) 런타임 경계 규칙

- Web 핸들러에서 SQLite 내부 직접 접근(`conn_ref`)을 금지합니다.
- Web 핸들러 데이터 접근은 storage adapter API를 통해서만 수행합니다.
- 무결성 민감 동작은 warn-and-continue가 아니라 fail-closed로 처리합니다.

### 4) 문서화 및 감사 추적성

- 무결성 정책 변경 시 본 문서와 `docs/security/integrity-runbook.md`를 함께 갱신해야 합니다.
- 취약점 신고/공개 정책은 `SECURITY.md`를 따릅니다.

## 로컬 검증 명령

```bash
./scripts/verify-integrity.sh
```

이 명령은 무결성 정책 테스트, 서명 검증 테스트, 공급망 검사, SBOM 생성을 한 번에 검증합니다.

## 키 롤오버 리허설

```bash
./scripts/rehearse-key-rotation.sh
```

이 스크립트는 `artifacts/integrity/key-rotation/`에 구키/신키 및 서명 아티팩트를 생성하여
이중 서명 전환 흐름을 사전에 검증할 수 있게 합니다.

## 미래 연동 준비 (Server / Third-Party)

standalone 단계에서는 즉시 필수는 아니지만, 이 단계부터 설계 제약으로 유지해야 합니다.

- 요청 계약에 `nonce`, `timestamp`, `key_id`, `sig` 필드 예약
- 재전송 방지(replay protection) 가능한 프로토콜 의미론 유지
- capability 기반 최소권한 서드파티 계약
- 루트키/온라인키 분리 및 키 롤오버 절차 문서화
