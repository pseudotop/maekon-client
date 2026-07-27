[English](./first-5-minutes.md) | [한국어](./first-5-minutes.ko.md)

# 첫 5분 가이드

Standalone 모드에서 MAEKON의 첫 유의미한 인사이트를 빠르게 얻기 위한 체크리스트입니다.

## 1. Standalone 모드 실행

```bash
cargo run -p maekon-app -- --offline
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

## 6. AI 기능 켜기 (선택)

AI 기능은 프라이버시 우선 원칙에 따라 **기본적으로 꺼져** 있습니다. 로컬 임베딩, AI 시맨틱 검색, AI 일일 다이제스트 내러티브를 켜려면 **Settings → Advanced → Enable AI features** 마스터 토글 하나를 켭니다(개별 토글은 고급 사용자를 위해 그대로 제공됩니다).

모든 처리는 **온디바이스**에서 수행됩니다. 로컬 임베딩이 벡터 인덱스를 만들고, 일일 다이제스트 내러티브는 로컬 LLM이 작성합니다.

참고: 이 설정은 시작 시 로드되므로 파이프라인이 적용되려면 **Maekon을 재시작**해야 합니다. 재시작 후 Search 페이지의 **Semantic** 모드를 선택할 수 있고, 일일 다이제스트에 AI가 작성한 내러티브가 추가됩니다.
