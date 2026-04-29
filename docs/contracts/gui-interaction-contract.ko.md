[English](./gui-interaction-contract.md) | [한국어](./gui-interaction-contract.ko.md)

# GUI 인터랙션 계약 (ADR-002)

GUI V2 인터랙션 API의 버전화된 HTTP 계약을 정의합니다
(`propose → highlight → confirm → execute` 상태 머신).

## 계약 버전

- 세션 페이로드: `automation.gui.v2`
- 이벤트 스트림 페이로드: `automation.gui.event.v1`
- 실행 티켓 페이로드: `automation.gui.ticket.v1`

## 호환성 규칙

1. 클라이언트는 `schema_version`이 있으면 반드시 읽고 분기해야 합니다.
2. 동일 버전 내 새 필드 추가는 하위 호환됩니다.
3. 필드 변경이 깨지는 경우 새 스키마 버전 문자열이 필요합니다.
4. `x-gui-session-token` 헤더는 세션 생성을 제외한 모든 엔드포인트에서 필수입니다.

## 엔드포인트

| 메서드 | 경로 | 설명 |
|--------|------|------|
| POST | `/api/automation/gui/sessions` | 세션 생성 (화면 캡처 + 요소 탐색) |
| GET | `/api/automation/gui/sessions/{id}` | 세션 상태 조회 |
| POST | `/api/automation/gui/sessions/{id}/highlight` | 후보 요소 하이라이트 |
| POST | `/api/automation/gui/sessions/{id}/confirm` | 후보 확인 및 실행 티켓 발급 |
| POST | `/api/automation/gui/sessions/{id}/execute` | 서명된 티켓으로 액션 실행 |
| DELETE | `/api/automation/gui/sessions/{id}` | 세션 취소 / 삭제 |
| GET | `/api/automation/gui/sessions/{id}/events` | SSE 이벤트 스트림 |

## 인증

`POST /sessions`를 제외한 모든 엔드포인트에 capability 토큰 헤더 필요:

```
x-gui-session-token: {token}
```

- 토큰은 `GuiCreateSessionResponse.capability_token`에서 반환됩니다.
- 빈 값 또는 공백만 있는 값은 `401 Unauthorized`로 거부됩니다.
- 토큰은 단일 세션에 한정됩니다.

## 상태 머신

```
Proposed ──► Highlighted ──► Confirmed ──► Executing ──► Executed
   │              │              │                          │
   └──────────────┴──────────────┴──► Cancelled             │
                                                            │
                  (TTL 만료) ──────────────────► Expired     │
```

허용되는 전이:

| From | To | 트리거 |
|------|----|--------|
| Proposed | Highlighted | `POST .../highlight` |
| Proposed | Confirmed | `POST .../confirm` (하이라이트 생략) |
| Highlighted | Confirmed | `POST .../confirm` |
| Confirmed | Executing | `POST .../execute` (내부) |
| Executing | Executed | 액션 완료 |
| Proposed / Highlighted / Confirmed | Cancelled | `DELETE .../sessions/{id}` |
| 비종료 상태 | Expired | 세션 TTL 초과 |

## 요청 / 응답 스키마

### POST `/api/automation/gui/sessions`

**요청** — `GuiCreateSessionRequest`:

```json
{
  "app_name": "string | null",
  "screen_id": "string | null",
  "min_confidence": "f64 | null (기본값 0.5)",
  "max_candidates": "usize | null (기본값 20)",
  "session_ttl_secs": "u64 | null (기본값 300)"
}
```

모든 필드 선택 사항. 빈 `{}` 본문으로 기본값 세션 생성 가능.

**응답 200** — `GuiCreateSessionResponse`:

```json
{
  "schema_version": "automation.gui.v2",
  "session": { "...GuiInteractionSession" },
  "capability_token": "string"
}
```

### GET `/api/automation/gui/sessions/{id}`

**응답 200** — `GuiSessionResponse`:

```json
{
  "schema_version": "automation.gui.v2",
  "session": { "...GuiInteractionSession" }
}
```

### POST `/api/automation/gui/sessions/{id}/highlight`

**요청** — `GuiHighlightRequest`:

```json
{
  "candidate_ids": ["string"] | null
}
```

`null`이면 모든 후보 하이라이트. 명시적 목록이면 해당 요소만.

**응답 200** — `GuiSessionResponse`.

### POST `/api/automation/gui/sessions/{id}/confirm`

**요청** — `GuiConfirmRequest`:

```json
{
  "candidate_id": "string",
  "action": {
    "action_type": "click | double_click | right_click | type_text",
    "text": "string | null"
  },
  "ticket_ttl_secs": "u64 | null (기본값 30)"
}
```

`action_type`이 `type_text`일 때 `text` 필수.

**응답 200** — `GuiConfirmResponse`:

```json
{
  "schema_version": "automation.gui.v2",
  "ticket": { "...GuiExecutionTicket" }
}
```

### POST `/api/automation/gui/sessions/{id}/execute`

**요청** — `GuiExecutionRequest`:

```json
{
  "ticket": { "...GuiExecutionTicket" }
}
```

티켓 객체는 confirm 응답에서 받은 것을 그대로 전달.

**응답 200** — `GuiExecuteResponse`:

```json
{
  "schema_version": "automation.gui.v2",
  "command_id": "string",
  "ticket": { "...GuiExecutionTicket" },
  "result": { "...IntentResult" },
  "outcome": {
    "session": { "...GuiInteractionSession" },
    "succeeded": true,
    "detail": "string | null"
  }
}
```

### DELETE `/api/automation/gui/sessions/{id}`

**응답 200** — `GuiSessionResponse` (state = `cancelled`).

### GET `/api/automation/gui/sessions/{id}/events`

**응답** — `text/event-stream` (SSE):

```
event: confirmed
data: {"schema_version":"automation.gui.event.v1","event_type":"confirmed","session_id":"...","state":"confirmed","emitted_at":"...","message":null}
```

Keep-alive: 15초마다 `ping`.

## 모델 정의

### GuiInteractionSession

| 필드 | 타입 | 비고 |
|------|------|------|
| `schema_version` | `String` | `"automation.gui.v2"` |
| `session_id` | `String` | UUID |
| `state` | `GuiSessionState` | 현재 상태 (상태 머신 참조) |
| `scene` | `UiScene` | 캡처된 UI 장면 |
| `focus` | `FocusSnapshot` | 캡처 시점 윈도우 포커스 |
| `candidates` | `Vec<GuiCandidate>` | 발견된 인터랙티브 요소 |
| `selected_element_id` | `String?` | confirm 후 설정됨 |
| `created_at` | `DateTime<Utc>` | ISO 8601 |
| `updated_at` | `DateTime<Utc>` | ISO 8601 |
| `expires_at` | `DateTime<Utc>` | ISO 8601 |

### GuiSessionState

```
proposed | highlighted | confirmed | executing | executed | cancelled | expired
```

### FocusSnapshot

| 필드 | 타입 | 비고 |
|------|------|------|
| `app_name` | `String` | 예: `"Code"` |
| `window_title` | `String` | 예: `"main.rs — VSCode"` |
| `pid` | `u32` | 프로세스 ID |
| `bounds` | `WindowBounds?` | `{x, y, width, height}` (nullable) |
| `captured_at` | `DateTime<Utc>` | ISO 8601 |
| `focus_hash` | `String` | 포커스 상태의 SHA-256 |

### GuiCandidate

| 필드 | 타입 | 비고 |
|------|------|------|
| `element` | `UiSceneElement` | 장면의 전체 요소 |
| `ranking_reason` | `String?` | 랭킹 이유 |
| `eligible` | `bool` | 인터랙션 가능 여부 |

### GuiActionType

```
click | double_click | right_click | type_text
```

### GuiExecutionTicket

| 필드 | 타입 | 비고 |
|------|------|------|
| `schema_version` | `String` | `"automation.gui.ticket.v1"` |
| `ticket_id` | `String` | UUID |
| `session_id` | `String` | 소속 세션 |
| `scene_id` | `String` | confirm 시점 장면 |
| `element_id` | `String` | 대상 요소 |
| `action_hash` | `String` | 확인된 액션의 SHA-256 |
| `focus_hash` | `String` | 드리프트 감지용 포커스 해시 |
| `issued_at` | `DateTime<Utc>` | ISO 8601 |
| `expires_at` | `DateTime<Utc>` | 티켓 만료 |
| `nonce` | `String` | 일회용 논스 |
| `signature` | `String` | HMAC-SHA256 서명 |

### GuiSessionEvent

| 필드 | 타입 | 비고 |
|------|------|------|
| `schema_version` | `String` | `"automation.gui.event.v1"` |
| `event_type` | `String` | 진입한 상태 이름 |
| `session_id` | `String` | 세션 UUID |
| `state` | `GuiSessionState` | 이벤트 시점 상태 |
| `emitted_at` | `DateTime<Utc>` | ISO 8601 |
| `message` | `String?` | 선택적 상세 |

### IntentResult

| 필드 | 타입 | 비고 |
|------|------|------|
| `success` | `bool` | 액션 성공 여부 |
| `element` | `UiElement?` | `{text, bounds, role, confidence, source}` |
| `verification` | `VerificationResult?` | `{screen_changed, changed_regions}` |
| `retry_count` | `u32` | 시도된 재시도 횟수 |
| `elapsed_ms` | `u64` | 실행 소요 시간 |
| `error` | `String?` | 실패 시 에러 메시지 |

## 티켓 보안

### HMAC 서명

- **알고리즘**: HMAC-SHA256
- **시크릿**: `ONESHIM_GUI_TICKET_HMAC_SECRET` 환경 변수
- **서명 내용**: `session_id|scene_id|element_id|action_hash|focus_hash|nonce`
- **검증**: 실행 시점에 서명을 재계산하여 비교

### 논스 재사용 방지

각 티켓 논스는 세션별로 추적됩니다. 이미 소비된 논스는
`422 Unprocessable` (`TicketInvalid`)로 거부됩니다.

### 포커스 드리프트 감지

실행 시점에 현재 윈도우 포커스를 재캡처하여 해시를 티켓의
`focus_hash`와 비교합니다. 불일치 시 `409 Conflict`
(`FocusDrift`)가 반환됩니다.

## 에러 매핑

| 도메인 에러 | HTTP | ApiError 변형 |
|------------|------|--------------|
| `Unauthorized` | 401 | `Unauthorized` |
| `NotFound(msg)` | 404 | `NotFound` |
| `BadRequest(msg)` | 400 | `BadRequest` |
| `Forbidden(msg)` | 403 | `Forbidden` |
| `FocusDrift(msg)` | 409 | `Conflict` |
| `TicketInvalid(msg)` | 422 | `Unprocessable` |
| `Unavailable(msg)` | 503 | `ServiceUnavailable` |
| `Internal(msg)` | 500 | `Internal` |

## 기본값

| 파라미터 | 기본값 | 범위 |
|----------|--------|------|
| `min_confidence` | 0.5 | 0.0–1.0 |
| `max_candidates` | 20 | ≥ 1 |
| `session_ttl_secs` | 300 | 초 |
| `ticket_ttl_secs` | 30 | 초 |
| 정리 주기 | 30 | 초 (내부) |
| 이벤트 채널 | 256 | 용량 (내부) |
| SSE keep-alive | 15 | 초 |
