[English](./ai-provider-contract.md) | [한국어](./ai-provider-contract.ko.md)

# AI Provider 계약 (Remote LLM/OCR)

이 문서는 `oneshim-network`의 원격 AI 어댑터가 기대하는 요청/응답 계약을 버전으로 정의합니다.

## 계약 버전

- `ai.provider.remote.v1`

## 범위

- `RemoteLlmProvider` (`crates/oneshim-network/src/ai_llm_client/` — ADR-003 디렉토리 모듈)
- `RemoteOcrProvider` (`crates/oneshim-network/src/ai_ocr_client/` — ADR-003 디렉토리 모듈)
- 어댑터 해석/폴백 (`src-tauri/src/provider_adapters/` — ADR-004 Tauri v2 마이그레이션으로 `crates/oneshim-app/` 에서 경로 변경된 디렉토리 모듈)

## 제공자 타입

`AiProviderType` 값:

- `anthropic`
- `openai`
- `google`
- `generic`

## Remote LLM 계약

## 요청 요구사항

1. 엔드포인트는 `http://` 또는 `https://`여야 합니다.
2. API 키는 비어 있지 않아야 합니다.
3. 타임아웃은 `>= 1`초여야 합니다.
4. 프롬프트 페이로드에는 다음이 포함되어야 합니다.
   - 활성 앱/창 메타데이터
   - 화면 텍스트 후보
   - 사용자 의도 힌트

## 응답 요구사항

최종 파싱 스키마:

```json
{
  "target_text": "optional string",
  "target_role": "optional string",
  "action_type": "click|type|hotkey|wait|activate",
  "confidence": 0.0
}
```

검증 규칙:

1. `action_type`은 비어 있으면 안 됩니다.
2. `confidence`는 유한 수치이며 `0.0..=1.0` 범위여야 합니다.

제공자별 파싱 경로:

- `anthropic`: `content[0].text`
- `openai`/`generic`: `choices[0].message.content`
- `google`: `candidates[0].content.parts[0].text`

텍스트에 markdown fence 같은 래퍼가 포함될 수 있으며, 어댑터는 첫 JSON 객체를 추출합니다.

## Remote OCR 계약

## 요청 요구사항

1. 이미지는 Base64와 media type(`image/png`, `image/jpeg`, `image/webp`)으로 전송합니다.
2. 엔드포인트/키/타임아웃 검증은 LLM과 동일합니다.
3. 외부 OCR 호출 전 Privacy Gateway 세정 경로를 반드시 통과해야 합니다.

## 응답 요구사항

최종 파싱 스키마:

```json
{
  "results": [
    {
      "text": "string",
      "x": 0,
      "y": 0,
      "width": 0,
      "height": 0,
      "confidence": 0.0
    }
  ]
}
```

검증 규칙:

1. `confidence`는 유한 수치이며 가능하면 `0.0..=1.0` 범위를 유지해야 합니다.
2. 소비자 계층은 저신뢰/잘못된 geometry 결과를 필터링할 수 있습니다.

제공자별 파싱 경로:

- `anthropic`: `content[].text` 줄 단위 추출
- `google`: `responses[0].textAnnotations[]` + bounding poly 변환
- `openai`: `choices[0].message.content`(또는 동등한 content 블록)을 파싱한 뒤:
  - JSON 객체(`{ "results": [...] }`) 우선 파싱
  - 구조화 JSON이 없으면 줄 단위 텍스트 파싱으로 폴백
- `generic`: `{ "results": [...] }` 직접 파싱을 우선하고, 실패 시 제공자 형식 폴백 파싱

## 실패 시맨틱

1. 2xx가 아닌 응답 => 어댑터 오류, 시맨틱 HTTP status 매핑 경로
   (wire code: `auth.failed`, `network.timeout`, `network.rate_limit`,
   `service.unavailable`, domain-fallback `provider.ocr_failed` /
   `provider.analysis_failed` / `network.generic`). 상세는
   [`docs/guides/http-status-error-mapping.ko.md`](../guides/http-status-error-mapping.ko.md) 참조.
2. 파싱 불일치 => 어댑터 오류.
   - **LLM**: `CoreError::Analysis { ProviderCode::AnalysisFailed }` (wire:
     `provider.analysis_failed`). iter-93 drift 수정 이후; 기존 `CoreError::Internal`은
     provider 이상 동작을 텔레메트리에서 숨겼음.
   - **OCR**: `CoreError::OcrError { ProviderCode::OcrFailed }` (wire:
     `provider.ocr_failed`).
3. `fallback_to_local=true`면 로컬 제공자로 폴백될 수 있습니다.
4. 폴백 비활성 시 원격 설정/초기화 오류는 fail-closed로 처리해야 합니다.

## 런타임 폴백 가시성

원격 어댑터 해석이 로컬로 폴백될 때, 런타임 상태는 API/SSE로 노출됩니다.

1. `GET /api/automation/status`
   - `ocr_source`, `llm_source` (`local|remote|local-fallback|cli-subscription|platform`)
   - `ocr_fallback_reason`, `llm_fallback_reason` (폴백이 없으면 `null`)
2. `GET /api/stream` 초기 이벤트
   - SSE 이벤트 타입: `ai_runtime_status`
   - 페이로드는 위 4개 런타임 필드와 동일

## CI live smoke 계약

수동 스모크 워크플로:

- `.github/workflows/ai-integration-smoke.yml`

실행 방식:

1. `workflow_dispatch` 전용
2. 저장소 시크릿으로 endpoint/key/model 주입
3. `crates/oneshim-network/tests/ai_provider_live_smoke.rs` 실행
4. OCR 스모크는 선택 실행 (`run_ocr` 입력)
5. 런타임 환경변수 이름(`ONESHIM_AI_SMOKE_LLM_*`, `ONESHIM_AI_SMOKE_OCR_*`)은 그대로 유지되며 하위 호환됩니다.

필수 시크릿:

- `ONESHIM_AI_SMOKE_LLM_ENDPOINT`
- `ONESHIM_AI_SMOKE_LLM_API_KEY`

선택 시크릿:

- `ONESHIM_AI_SMOKE_LLM_MODEL`
- `ONESHIM_AI_SMOKE_OCR_ENDPOINT` (`google` OCR provider 선택 시 필수)
- `ONESHIM_AI_SMOKE_OCR_API_KEY` (선택, 없으면 LLM API key로 폴백)
- `ONESHIM_AI_SMOKE_OCR_MODEL`

live smoke OCR 폴백 규칙:

- OCR provider type 미지정 시 LLM provider type을 기본값으로 사용합니다.
- OCR timeout 미지정 시 LLM timeout을 기본값으로 사용합니다.
- `anthropic` / `openai` / `generic`는 OCR endpoint 미지정 시 LLM endpoint로 폴백합니다.
- `google`은 OCR이 Vision API 계약(`/v1/images:annotate`)을 사용하므로 endpoint 폴백을 허용하지 않습니다.
- `google` 외 provider는 OCR model 미지정 시 LLM model을 상속할 수 있습니다.
