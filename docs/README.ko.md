[English](./README.md) | [한국어](./README.ko.md)

# 문서 인덱스

이 디렉터리는 문서 목적에 따라 구성됩니다.

## 루트 문서

- [DOCUMENTATION_POLICY.md](./DOCUMENTATION_POLICY.md): 문서 컨벤션 및 유지보수 규칙
- [install.ko.md](./install.ko.md): 설치 가이드
- [testing/source-build-prerequisites.md](./testing/source-build-prerequisites.md): fresh checkout 빌드 및 Tauri sidecar 준비 조건

## 하위 디렉터리

- `architecture/`: ADR 전용 아키텍처 결정 문서
- `guides/`: 운영/개발 플레이북, 런북, how-to 가이드
- `contracts/`: 버전드 API/payload 계약 문서와 생성 OpenAPI 스냅샷
- `crates/`: crate 단위 구현 레퍼런스
- `security/`: 보안 기준선 및 무결성 운영 문서
- `qa/`: QA 템플릿, 실행 기록, 아티팩트 메타 문서
- `testing/`: 테스트 전략 문서

내부 planning, research, review, roadmap, migration archive 는
public-minimal export 에 포함하지 않습니다. 공개 contributor 에게 필요한
영속적 결정은 ADR, guide, contract, security 문서로 승격합니다.

## 아키텍처 및 색인 정책

- 공개 독자는 ADR을 보려면 `docs/architecture/README.md`에서 시작하고,
  shipped behavior는 `docs/guides/`, `docs/contracts/`, `docs/security/`,
  crate 문서에서 확인합니다.
- 내부 유지보수자는 TC catalog 백필이나 아키텍처 승격 전에 parent SSOT의
  plan index를 먼저 확인합니다. 이 내부 색인은 공개 export에서 빠지는
  planning, research, private TC 기록을 직접 가리킬 수 있습니다.
- 공개 문서와 export source comment는 자체 완결적이어야 합니다. source
  comment에 영속 설명이 필요하면 먼저 public ADR, guide, contract,
  security doc, crate doc으로 승격합니다.

## 빠른 배치 규칙

1. `docs/architecture/`에는 `ADR-XXX-*` 형식만 둡니다.
2. 절차형 플레이북/런북은 `docs/guides/`에 두고, 보안 전용이면 `docs/security/`에 둡니다.
3. API 와 payload contract 는 `docs/contracts/`에 둡니다.
4. 공개 핵심 문서는 영문 기본 + 한국어 companion을 함께 유지합니다.
