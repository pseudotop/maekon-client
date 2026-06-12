# ai_model_lifecycle_policy.json — 분기별 유지 관리 가이드

F-RC-C23-04 (cycle 23 W2 Bundle D): 카탈로그 갱신 정책 명문화.

## 갱신 주기

분기마다 (1월, 4월, 7월, 10월 초) 아래 항목을 확인한다:

1. 업스트림 공급사 공식 발표에서 새로운 모델 deprecation 일정을 확인한다.
   - OpenAI: https://platform.openai.com/docs/deprecations
   - Anthropic: https://docs.anthropic.com/en/api/versioning
   - Google: https://cloud.google.com/vertex-ai/generative-ai/docs/deprecations

2. `block_at` 날짜가 이미 지난 항목은 `action` 을 `"blocked"` 로 변경한다.

3. 신규 deprecation 항목을 추가할 때는 `warn_at` 을 `block_at` 보다 최소 90일 앞서 설정한다.

4. `updated_at` 필드를 갱신 일시(UTC ISO 8601)로 업데이트한다.

5. PR 본문에 변경 이유와 공급사 공지 링크를 포함한다.

## 현재 차단 모델 (2026-05-24 기준)

| 공급사 | 모델 | block_at | 대체 모델 |
|--------|------|----------|----------|
| OpenAI | gpt-3.5-turbo | 2026-01-01 | gpt-5.4 |
| Anthropic | claude-3-sonnet-20240229 | 2026-04-01 | claude-sonnet-4-20250514 |
| Google | gemini-1.5-pro | 2026-06-01 | gemini-2.5-pro |
