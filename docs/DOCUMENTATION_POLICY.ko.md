[English](./DOCUMENTATION_POLICY.md) | [한국어](./DOCUMENTATION_POLICY.ko.md)

# 문서 정책

기준 문서는 영문 기본 문서인 [DOCUMENTATION_POLICY.md](./DOCUMENTATION_POLICY.md)입니다.

## 언어 정책

- 공개 문서는 **영문 기본(English-primary)** 으로 운영합니다.
- 핵심 공개 문서에는 한국어 companion 문서를 함께 유지합니다.
- 한국어 companion은 영문 기본 문서와 의미를 동기화합니다.

## 지표 일관성 정책

- 변동 지표는 문서 곳곳에 중복 기재하지 않습니다.
- 실시간 워크플로우 상태의 기준은 문서가 아니라 GitHub Actions run 페이지입니다.

## 디렉터리 구조 정책

- `docs/architecture/`는 ADR 전용입니다.
  - 파일 이름 규칙: `ADR-XXX-*.md`, `ADR-XXX-*.ko.md`
  - 리서치/플레이북/런북 같은 비-ADR 문서는 이 디렉터리에 두지 않습니다.
- `docs/guides/`는 운영/개발 플레이북과 how-to/런북 문서를 관리합니다.
- `docs/contracts/`는 버전드 payload/API 계약 문서와 생성 OpenAPI 스냅샷을 관리합니다.
- `docs/crates/`는 crate별 구현 레퍼런스를 관리합니다.
- `docs/security/`는 보안 기준선 및 무결성 운영 문서를 관리합니다.
- `docs/qa/`는 QA 템플릿과 실행 기록 메타 문서를 관리합니다.
- `docs/testing/`는 테스트 전략 문서를 관리합니다.
- 내부 planning, research, review, roadmap, migration, session workflow artifact 는 public-minimal export 에 포함하지 않습니다. 공개 결정으로 남길 내용은 `docs/architecture/`, `docs/guides/`, `docs/contracts/`, `docs/security/` 로 승격합니다.

현재 문서 맵은 [docs/README.ko.md](./README.ko.md)를 참조하세요.
