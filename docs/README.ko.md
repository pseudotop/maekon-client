[English](./README.md) | [한국어](./README.ko.md)

# 문서 인덱스

이 디렉터리는 문서 목적에 따라 구성됩니다.

## 루트 문서

- [DOCUMENTATION_POLICY.md](./DOCUMENTATION_POLICY.md): 문서 컨벤션 및 유지보수 규칙
- [install.ko.md](./install.ko.md): 설치 가이드

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

## 빠른 배치 규칙

1. `docs/architecture/`에는 `ADR-XXX-*` 형식만 둡니다.
2. 절차형 플레이북/런북은 `docs/guides/`에 두고, 보안 전용이면 `docs/security/`에 둡니다.
3. API 와 payload contract 는 `docs/contracts/`에 둡니다.
4. 공개 핵심 문서는 영문 기본 + 한국어 companion을 함께 유지합니다.
