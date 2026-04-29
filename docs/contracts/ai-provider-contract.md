[English](./ai-provider-contract.md) | [한국어](./ai-provider-contract.ko.md)

# AI Provider Contract (Remote LLM/OCR)

This document defines the versioned request/response contract expected by the remote AI adapters in `oneshim-network`.

## Contract Version

- `ai.provider.remote.v1`

## Scope

- `RemoteLlmProvider` (`crates/oneshim-network/src/ai_llm_client/` — directory module per ADR-003)
- `RemoteOcrProvider` (`crates/oneshim-network/src/ai_ocr_client/` — directory module per ADR-003)
- Adapter resolution and fallback (`src-tauri/src/provider_adapters/` — directory module; path changed from `crates/oneshim-app/` per ADR-004 Tauri v2 migration)

## Provider Types

`AiProviderType` values:

- `anthropic`
- `openai`
- `google`
- `generic`

## Remote LLM Contract

## Request requirements

1. Endpoint must be `http://` or `https://`.
2. API key must be non-empty.
3. Timeout must be `>= 1` seconds.
4. Prompt payload must include:
   - Active app/window metadata
   - Visible text candidates
   - User intent hint

## Response requirements

Final parsed schema:

```json
{
  "target_text": "optional string",
  "target_role": "optional string",
  "action_type": "click|type|hotkey|wait|activate",
  "confidence": 0.0
}
```

Validation rules:

1. `action_type` MUST be non-empty.
2. `confidence` MUST be finite and in `0.0..=1.0`.

Provider-specific parsing path:

- `anthropic`: parse `content[0].text`
- `openai`/`generic`: parse `choices[0].message.content`
- `google`: parse `candidates[0].content.parts[0].text`

Text payloads MAY include wrappers (for example markdown fences); adapters extract the first JSON object body.

## Remote OCR Contract

## Request requirements

1. Image is sent as Base64 with media type (`image/png`, `image/jpeg`, or `image/webp`).
2. Same endpoint/key/timeout validation as LLM.
3. External OCR calls MUST pass privacy gateway sanitization first.

## Response requirements

Final parsed schema:

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

Validation rules:

1. `confidence` SHOULD be finite and in `0.0..=1.0`.
2. Consumers MAY filter out low-confidence or invalid geometry results.

Provider-specific parsing path:

- `anthropic`: line extraction from `content[].text`
- `google`: `responses[0].textAnnotations[]` + bounding poly conversion
- `openai`: parse `choices[0].message.content` (or equivalent content blocks), then:
  - prefer JSON object parse (`{ "results": [...] }`)
  - fallback to line-by-line text parse when structured JSON is not returned
- `generic`: prefer direct parse of `{ "results": [...] }`, then provider-format fallback parsing

## Failure semantics

1. Non-2xx response => adapter error, routed through semantic HTTP status mapping
   (wire codes: `auth.failed`, `network.timeout`, `network.rate_limit`,
   `service.unavailable`, domain-fallback `provider.ocr_failed` /
   `provider.analysis_failed` / `network.generic`). See
   [`docs/guides/http-status-error-mapping.md`](../guides/http-status-error-mapping.md).
2. Parse mismatch => adapter error.
   - **LLM**: `CoreError::Analysis { ProviderCode::AnalysisFailed }` (wire:
     `provider.analysis_failed`). Post iter-93 drift fix; previously was
     `CoreError::Internal` which hid provider misbehaviour in telemetry.
   - **OCR**: `CoreError::OcrError { ProviderCode::OcrFailed }` (wire:
     `provider.ocr_failed`).
3. When fallback is enabled (`fallback_to_local=true`), adapter resolution
   MAY switch to local providers.
4. When fallback is disabled, invalid remote config or adapter init errors
   MUST fail closed.

## Runtime fallback visibility

When remote adapter resolution falls back to local, runtime status is surfaced through API and stream payloads:

1. `GET /api/automation/status`
   - `ocr_source`, `llm_source` (`local|remote|local-fallback|cli-subscription|platform`)
   - `ocr_fallback_reason`, `llm_fallback_reason` (`null` when no fallback happened)
2. `GET /api/stream` bootstrap event
   - SSE event type: `ai_runtime_status`
   - payload mirrors the same four runtime fields

## CI live smoke contract

Manual smoke workflow:

- `.github/workflows/ai-integration-smoke.yml`

Execution mode:

1. `workflow_dispatch` only
2. Uses repository secrets for endpoint/key/model
3. Runs `crates/oneshim-network/tests/ai_provider_live_smoke.rs`
4. OCR smoke is optional (`run_ocr` input)
5. Runtime env names stay split (`ONESHIM_AI_SMOKE_LLM_*`, `ONESHIM_AI_SMOKE_OCR_*`) and are backward compatible.

Required secrets:

- `ONESHIM_AI_SMOKE_LLM_ENDPOINT`
- `ONESHIM_AI_SMOKE_LLM_API_KEY`

Optional secrets:

- `ONESHIM_AI_SMOKE_LLM_MODEL`
- `ONESHIM_AI_SMOKE_OCR_ENDPOINT` (required when OCR provider type is `google`)
- `ONESHIM_AI_SMOKE_OCR_API_KEY` (optional; falls back to LLM API key)
- `ONESHIM_AI_SMOKE_OCR_MODEL`

OCR fallback rules in live smoke:

- OCR provider type defaults to LLM provider type when omitted.
- OCR timeout defaults to LLM timeout when omitted.
- OCR endpoint falls back to LLM endpoint for `anthropic` / `openai` / `generic`.
- OCR endpoint fallback is not allowed for `google` because OCR expects Vision API contract (`/v1/images:annotate`).
- OCR model may inherit LLM model for non-`google` providers.
