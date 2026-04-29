[English](./gui-interaction-contract.md) | [한국어](./gui-interaction-contract.ko.md)

# GUI Interaction Contract (ADR-002)

This document defines the versioned HTTP contract for the GUI V2 interaction API
(`propose → highlight → confirm → execute` state machine).

## Contract versions

- Session payload: `automation.gui.v2`
- Event stream payload: `automation.gui.event.v1`
- Execution ticket payload: `automation.gui.ticket.v1`

## Compatibility rules

1. Clients MUST read and branch on `schema_version` when present.
2. New additive fields are backward compatible inside the same version.
3. Breaking field changes require a new schema version string.
4. The `x-gui-session-token` header is mandatory for all endpoints except session creation.

## Endpoints

| Method | Path | Description |
|--------|------|-------------|
| POST | `/api/automation/gui/sessions` | Create session (scene capture + element discovery) |
| GET | `/api/automation/gui/sessions/{id}` | Retrieve session state |
| POST | `/api/automation/gui/sessions/{id}/highlight` | Highlight candidate elements |
| POST | `/api/automation/gui/sessions/{id}/confirm` | Confirm candidate and issue execution ticket |
| POST | `/api/automation/gui/sessions/{id}/execute` | Execute action using signed ticket |
| DELETE | `/api/automation/gui/sessions/{id}` | Cancel / delete session |
| GET | `/api/automation/gui/sessions/{id}/events` | SSE event stream |

## Authentication

All endpoints except `POST /sessions` require the capability token header:

```
x-gui-session-token: {token}
```

- The token is returned in `GuiCreateSessionResponse.capability_token`.
- Empty or whitespace-only values are rejected with `401 Unauthorized`.
- Tokens are scoped to a single session.

## State machine

```
Proposed ──► Highlighted ──► Confirmed ──► Executing ──► Executed
   │              │              │                          │
   └──────────────┴──────────────┴──► Cancelled             │
                                                            │
                  (TTL expiry) ──────────────────► Expired   │
```

Allowed transitions:

| From | To | Trigger |
|------|----|---------|
| Proposed | Highlighted | `POST .../highlight` |
| Proposed | Confirmed | `POST .../confirm` (skip highlight) |
| Highlighted | Confirmed | `POST .../confirm` |
| Confirmed | Executing | `POST .../execute` (internal) |
| Executing | Executed | Action completes |
| Proposed / Highlighted / Confirmed | Cancelled | `DELETE .../sessions/{id}` |
| Any non-terminal | Expired | Session TTL exceeded |

## Request / Response schemas

### POST `/api/automation/gui/sessions`

**Request** — `GuiCreateSessionRequest`:

```json
{
  "app_name": "string | null",
  "screen_id": "string | null",
  "min_confidence": "f64 | null (default 0.5)",
  "max_candidates": "usize | null (default 20)",
  "session_ttl_secs": "u64 | null (default 300)"
}
```

All fields are optional. An empty `{}` body creates a session with defaults.

**Response 200** — `GuiCreateSessionResponse`:

```json
{
  "schema_version": "automation.gui.v2",
  "session": { "...GuiInteractionSession" },
  "capability_token": "string"
}
```

### GET `/api/automation/gui/sessions/{id}`

**Response 200** — `GuiSessionResponse`:

```json
{
  "schema_version": "automation.gui.v2",
  "session": { "...GuiInteractionSession" }
}
```

### POST `/api/automation/gui/sessions/{id}/highlight`

**Request** — `GuiHighlightRequest`:

```json
{
  "candidate_ids": ["string"] | null
}
```

`null` highlights all candidates. An explicit list highlights only those elements.

**Response 200** — `GuiSessionResponse`.

### POST `/api/automation/gui/sessions/{id}/confirm`

**Request** — `GuiConfirmRequest`:

```json
{
  "candidate_id": "string",
  "action": {
    "action_type": "click | double_click | right_click | type_text",
    "text": "string | null"
  },
  "ticket_ttl_secs": "u64 | null (default 30)"
}
```

`text` is required when `action_type` is `type_text`.

**Response 200** — `GuiConfirmResponse`:

```json
{
  "schema_version": "automation.gui.v2",
  "ticket": { "...GuiExecutionTicket" }
}
```

### POST `/api/automation/gui/sessions/{id}/execute`

**Request** — `GuiExecutionRequest`:

```json
{
  "ticket": { "...GuiExecutionTicket" }
}
```

The ticket object must be passed verbatim from the confirm response.

**Response 200** — `GuiExecuteResponse`:

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

**Response 200** — `GuiSessionResponse` (state = `cancelled`).

### GET `/api/automation/gui/sessions/{id}/events`

**Response** — `text/event-stream` (SSE):

```
event: confirmed
data: {"schema_version":"automation.gui.event.v1","event_type":"confirmed","session_id":"...","state":"confirmed","emitted_at":"...","message":null}
```

Keep-alive: `ping` every 15 seconds.

## Model definitions

### GuiInteractionSession

| Field | Type | Notes |
|-------|------|-------|
| `schema_version` | `String` | `"automation.gui.v2"` |
| `session_id` | `String` | UUID |
| `state` | `GuiSessionState` | Current state (see state machine) |
| `scene` | `UiScene` | Captured UI scene |
| `focus` | `FocusSnapshot` | Window focus at capture time |
| `candidates` | `Vec<GuiCandidate>` | Interactive elements found |
| `selected_element_id` | `String?` | Set after confirm |
| `created_at` | `DateTime<Utc>` | ISO 8601 |
| `updated_at` | `DateTime<Utc>` | ISO 8601 |
| `expires_at` | `DateTime<Utc>` | ISO 8601 |

### GuiSessionState

```
proposed | highlighted | confirmed | executing | executed | cancelled | expired
```

### FocusSnapshot

| Field | Type | Notes |
|-------|------|-------|
| `app_name` | `String` | e.g. `"Code"` |
| `window_title` | `String` | e.g. `"main.rs — VSCode"` |
| `pid` | `u32` | Process ID |
| `bounds` | `WindowBounds?` | `{x, y, width, height}` (nullable) |
| `captured_at` | `DateTime<Utc>` | ISO 8601 |
| `focus_hash` | `String` | SHA-256 of focus state |

### GuiCandidate

| Field | Type | Notes |
|-------|------|-------|
| `element` | `UiSceneElement` | Full element from scene |
| `ranking_reason` | `String?` | Why ranked |
| `eligible` | `bool` | Can be interacted with |

### GuiActionType

```
click | double_click | right_click | type_text
```

### GuiExecutionTicket

| Field | Type | Notes |
|-------|------|-------|
| `schema_version` | `String` | `"automation.gui.ticket.v1"` |
| `ticket_id` | `String` | UUID |
| `session_id` | `String` | Owning session |
| `scene_id` | `String` | Scene at confirm time |
| `element_id` | `String` | Target element |
| `action_hash` | `String` | SHA-256 of confirmed action |
| `focus_hash` | `String` | Focus hash for drift detection |
| `issued_at` | `DateTime<Utc>` | ISO 8601 |
| `expires_at` | `DateTime<Utc>` | Ticket expiry |
| `nonce` | `String` | One-time use nonce |
| `signature` | `String` | HMAC-SHA256 signature |

### GuiSessionEvent

| Field | Type | Notes |
|-------|------|-------|
| `schema_version` | `String` | `"automation.gui.event.v1"` |
| `event_type` | `String` | State name that was entered |
| `session_id` | `String` | Session UUID |
| `state` | `GuiSessionState` | State at event time |
| `emitted_at` | `DateTime<Utc>` | ISO 8601 |
| `message` | `String?` | Optional detail |

### IntentResult

| Field | Type | Notes |
|-------|------|-------|
| `success` | `bool` | Whether action succeeded |
| `element` | `UiElement?` | `{text, bounds, role, confidence, source}` |
| `verification` | `VerificationResult?` | `{screen_changed, changed_regions}` |
| `retry_count` | `u32` | Retries attempted |
| `elapsed_ms` | `u64` | Execution duration |
| `error` | `String?` | Error message if failed |

## Ticket security

### HMAC signing

- **Algorithm**: HMAC-SHA256
- **Secret**: `ONESHIM_GUI_TICKET_HMAC_SECRET` environment variable
- **Signed content**: `session_id|scene_id|element_id|action_hash|focus_hash|nonce`
- **Validation**: Signature is recomputed at execute time and compared

### Nonce replay protection

Each ticket nonce is tracked per session. A nonce that has already been consumed
is rejected with `422 Unprocessable` (`TicketInvalid`).

### Focus drift detection

At execute time the current window focus is re-captured and its hash is compared
against the ticket's `focus_hash`. A mismatch results in `409 Conflict`
(`FocusDrift`).

## Error mapping

| Domain Error | HTTP | ApiError variant |
|-------------|------|-----------------|
| `Unauthorized` | 401 | `Unauthorized` |
| `NotFound(msg)` | 404 | `NotFound` |
| `BadRequest(msg)` | 400 | `BadRequest` |
| `Forbidden(msg)` | 403 | `Forbidden` |
| `FocusDrift(msg)` | 409 | `Conflict` |
| `TicketInvalid(msg)` | 422 | `Unprocessable` |
| `Unavailable(msg)` | 503 | `ServiceUnavailable` |
| `Internal(msg)` | 500 | `Internal` |

## Defaults

| Parameter | Default | Range |
|-----------|---------|-------|
| `min_confidence` | 0.5 | 0.0–1.0 |
| `max_candidates` | 20 | ≥ 1 |
| `session_ttl_secs` | 300 | seconds |
| `ticket_ttl_secs` | 30 | seconds |
| Cleanup interval | 30 | seconds (internal) |
| Event channel | 256 | capacity (internal) |
| SSE keep-alive | 15 | seconds |
