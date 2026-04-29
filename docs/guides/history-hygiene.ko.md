[English](./history-hygiene.md) | [한국어](./history-hygiene.ko.md)

# 히스토리 위생 가이드

이 문서는 릴리스 안정성과 민감 맥락 추상화를 위한 커밋 히스토리 관리 기준을 정의합니다.

## 머지 정책

1. `main`으로 들어가는 기능/수정 PR은 squash merge를 기본으로 사용합니다.
2. 머지 커밋 제목은 의도 중심으로 추상화하고, 시크릿/설정 상세를 직접 노출하지 않습니다.
3. 상세 실행 로그는 커밋 제목이 아니라 CI 아티팩트와 런북에 남깁니다.

## 민감 맥락 규칙

커밋 제목에 아래 내용은 포함하지 않습니다.

- raw env 대입 형태 (`ONESHIM_...=...`, `MACOS_...=...`)
- 직접 시크릿 노출을 연상시키는 표현 (`password`, `private key`, `p12`, `token=`)
- 불필요한 계정 식별자

대신 다음과 같이 추상화합니다.

- `auth credential handling`
- `notarization profile wiring`
- `remote provider env alignment`

## CI 강제 규칙

- `scripts/verify-commit-message-hygiene.sh`로 신규 커밋 제목의 민감 키워드/환경변수 대입 패턴을 검사합니다.
- `scripts/verify-http-interface-manifest.sh`로 공개 HTTP 인터페이스 변경 이력을 버전드 매니페스트와 동기화합니다.
- `scripts/verify-http-openapi-sync.sh`로 생성 OpenAPI 스냅샷과 인터페이스 매니페스트의 동기화를 강제합니다.

## 운영 참고

이미 배포된 `main`의 히스토리 재작성은 기본적으로 지양합니다.
과거 이력 정정이 불가피하면 force-push 승인 및 하위 사용자 공지를 포함한 별도 인시던트 절차로 진행합니다.
