[English](./first-5-minutes.md) | [한국어](./first-5-minutes.ko.md)

# 첫 5분 가이드

Standalone 모드에서 ONESHIM의 첫 유의미한 인사이트를 빠르게 얻기 위한 체크리스트입니다.

## 1. Standalone 모드 실행

```bash
cargo run -p oneshim-app -- --offline
```

기대 결과: 서버/인증 의존 없이 앱이 시작됩니다.

## 2. 로컬 대시보드 접속

- URL: `http://localhost:10090`
- 대시보드 패널(메트릭, 타임라인, 집중도)이 정상 로드되는지 확인합니다.

## 3. 프라이버시 기본선 유지

Settings에서:
- 샌드박스를 `Standard` 또는 `Strict`로 유지
- `external_data_policy`를 `PiiFilterStandard` 이상으로 유지
- `allow_unredacted_external_ocr=false` 유지

## 4. 워크플로우 프리셋 1개 실행

우선 아래 프리셋 중 1개를 실행합니다.
- `daily-priority-sync`
- `deep-work-start`

기대 결과: 자동화 감사 로그에 성공/차단 신호가 기록됩니다.

## 5. 첫 진단 번들 확보

다음 API를 조회합니다.
- `GET /api/onboarding/quickstart`
- `GET /api/support/diagnostics`
- `GET /api/automation/policy-events?limit=50`

기대 결과: 설정/헬스/정책 액션 스냅샷을 확보해 재현 가능한 개선 루프를 만들 수 있습니다.
