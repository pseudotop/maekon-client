# GUI Interaction V2 API Examples

Concrete request/response examples for all 7 GUI V2 endpoints. Schemas are defined in
[gui-interaction-contract.md](./gui-interaction-contract.md).

Base URL: `http://localhost:10090`

---

## 1. POST /api/automation/gui/sessions

Create a new GUI interaction session (scene capture + element discovery).

### Request

```bash
curl -s -X POST http://localhost:10090/api/automation/gui/sessions \
  -H "Content-Type: application/json" \
  -d '{
    "app_name": "Code",
    "screen_id": null,
    "min_confidence": 0.6,
    "max_candidates": 10,
    "session_ttl_secs": 300
  }'
```

Minimal request (all defaults):

```bash
curl -s -X POST http://localhost:10090/api/automation/gui/sessions \
  -H "Content-Type: application/json" \
  -d '{}'
```

### Success Response (200)

```json
{
  "schema_version": "automation.gui.v2",
  "session": {
    "schema_version": "automation.gui.v2",
    "session_id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
    "state": "proposed",
    "scene": {
      "scene_id": "scene-20260320-143022-001",
      "elements": [
        {
          "element_id": "elem-001",
          "role": "button",
          "text_masked": "Save File",
          "confidence": 0.92,
          "label": "Save File",
          "bbox_abs": { "x": 120, "y": 45, "width": 80, "height": 32 },
          "bbox_norm": { "x": 0.0625, "y": 0.0417, "width": 0.0417, "height": 0.0296 }
        },
        {
          "element_id": "elem-002",
          "role": "text_field",
          "text_masked": "Search...",
          "confidence": 0.88,
          "label": "Search...",
          "bbox_abs": { "x": 300, "y": 10, "width": 200, "height": 28 },
          "bbox_norm": { "x": 0.1563, "y": 0.0093, "width": 0.1042, "height": 0.0259 }
        }
      ],
      "captured_at": "2026-03-20T14:30:22.456Z"
    },
    "focus": {
      "app_name": "Code",
      "window_title": "main.rs — VSCode",
      "pid": 12345,
      "bounds": { "x": 0, "y": 0, "width": 1920, "height": 1080 },
      "captured_at": "2026-03-20T14:30:22.400Z",
      "focus_hash": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    },
    "candidates": [
      {
        "element": {
          "element_id": "elem-001",
          "role": "button",
          "text_masked": "Save File",
          "confidence": 0.92,
          "label": "Save File",
          "bbox_abs": { "x": 120, "y": 45, "width": 80, "height": 32 },
          "bbox_norm": { "x": 0.0625, "y": 0.0417, "width": 0.0417, "height": 0.0296 }
        },
        "ranking_reason": "High confidence interactive button",
        "eligible": true
      },
      {
        "element": {
          "element_id": "elem-002",
          "role": "text_field",
          "text_masked": "Search...",
          "confidence": 0.88,
          "label": "Search...",
          "bbox_abs": { "x": 300, "y": 10, "width": 200, "height": 28 },
          "bbox_norm": { "x": 0.1563, "y": 0.0093, "width": 0.1042, "height": 0.0259 }
        },
        "ranking_reason": "Text input field",
        "eligible": true
      }
    ],
    "selected_element_id": null,
    "created_at": "2026-03-20T14:30:22.500Z",
    "updated_at": "2026-03-20T14:30:22.500Z",
    "expires_at": "2026-03-20T14:35:22.500Z"
  },
  "capability_token": "b7f3a1d9c8e2f4a6b0d1e3f5a7c9d2e4f6a8b0c2d4e6f8a0b2c4d6e8f0a2b4"
}
```

### Error Response (503) -- Missing HMAC secret

```json
{
  "error": "ServiceUnavailable",
  "message": "ONESHIM_GUI_TICKET_HMAC_SECRET is missing or empty"
}
```

### Error Response (400) -- No candidates found

```json
{
  "error": "BadRequest",
  "message": "No eligible GUI candidates found in scene"
}
```

---

## 2. GET /api/automation/gui/sessions/{id}

Retrieve session state.

### Request

```bash
curl -s http://localhost:10090/api/automation/gui/sessions/a1b2c3d4-e5f6-7890-abcd-ef1234567890 \
  -H "x-gui-session-token: b7f3a1d9c8e2f4a6b0d1e3f5a7c9d2e4f6a8b0c2d4e6f8a0b2c4d6e8f0a2b4"
```

### Success Response (200)

```json
{
  "schema_version": "automation.gui.v2",
  "session": {
    "schema_version": "automation.gui.v2",
    "session_id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
    "state": "proposed",
    "scene": { "...scene object..." },
    "focus": { "...focus snapshot..." },
    "candidates": [ "...candidate list..." ],
    "selected_element_id": null,
    "created_at": "2026-03-20T14:30:22.500Z",
    "updated_at": "2026-03-20T14:30:22.500Z",
    "expires_at": "2026-03-20T14:35:22.500Z"
  }
}
```

### Error Response (401) -- Missing or invalid token

```json
{
  "error": "Unauthorized",
  "message": "Unauthorized"
}
```

### Error Response (404) -- Session not found

```json
{
  "error": "NotFound",
  "message": "a1b2c3d4-e5f6-7890-abcd-ef1234567890"
}
```

---

## 3. POST /api/automation/gui/sessions/{id}/highlight

Highlight candidate elements with an overlay.

### Request -- Specific candidates

```bash
curl -s -X POST http://localhost:10090/api/automation/gui/sessions/a1b2c3d4-e5f6-7890-abcd-ef1234567890/highlight \
  -H "Content-Type: application/json" \
  -H "x-gui-session-token: b7f3a1d9c8e2f4a6b0d1e3f5a7c9d2e4f6a8b0c2d4e6f8a0b2c4d6e8f0a2b4" \
  -d '{
    "candidate_ids": ["elem-001", "elem-002"]
  }'
```

### Request -- Highlight all candidates

```bash
curl -s -X POST http://localhost:10090/api/automation/gui/sessions/a1b2c3d4-e5f6-7890-abcd-ef1234567890/highlight \
  -H "Content-Type: application/json" \
  -H "x-gui-session-token: b7f3a1d9c8e2f4a6b0d1e3f5a7c9d2e4f6a8b0c2d4e6f8a0b2c4d6e8f0a2b4" \
  -d '{
    "candidate_ids": null
  }'
```

### Success Response (200)

```json
{
  "schema_version": "automation.gui.v2",
  "session": {
    "session_id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
    "state": "highlighted",
    "updated_at": "2026-03-20T14:30:28.100Z",
    "...remaining session fields..."
  }
}
```

### Error Response (400) -- No highlight targets

```json
{
  "error": "BadRequest",
  "message": "No highlight targets available"
}
```

### Error Response (404) -- Session not found

```json
{
  "error": "NotFound",
  "message": "a1b2c3d4-e5f6-7890-abcd-ef1234567890"
}
```

---

## 4. POST /api/automation/gui/sessions/{id}/confirm

Confirm a candidate and issue an execution ticket.

### Request

```bash
curl -s -X POST http://localhost:10090/api/automation/gui/sessions/a1b2c3d4-e5f6-7890-abcd-ef1234567890/confirm \
  -H "Content-Type: application/json" \
  -H "x-gui-session-token: b7f3a1d9c8e2f4a6b0d1e3f5a7c9d2e4f6a8b0c2d4e6f8a0b2c4d6e8f0a2b4" \
  -d '{
    "candidate_id": "elem-001",
    "action": {
      "action_type": "click"
    },
    "ticket_ttl_secs": 30
  }'
```

### Request -- type_text action

```bash
curl -s -X POST http://localhost:10090/api/automation/gui/sessions/a1b2c3d4-e5f6-7890-abcd-ef1234567890/confirm \
  -H "Content-Type: application/json" \
  -H "x-gui-session-token: b7f3a1d9c8e2f4a6b0d1e3f5a7c9d2e4f6a8b0c2d4e6f8a0b2c4d6e8f0a2b4" \
  -d '{
    "candidate_id": "elem-002",
    "action": {
      "action_type": "type_text",
      "text": "hello world"
    },
    "ticket_ttl_secs": 30
  }'
```

### Success Response (200)

```json
{
  "schema_version": "automation.gui.v2",
  "ticket": {
    "schema_version": "automation.gui.ticket.v1",
    "ticket_id": "f1e2d3c4-b5a6-9870-fedc-ba0987654321",
    "session_id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
    "scene_id": "scene-20260320-143022-001",
    "element_id": "elem-001",
    "action_hash": "a9f8e7d6c5b4a3f2e1d0c9b8a7f6e5d4c3b2a1f0e9d8c7b6a5f4e3d2c1b0a9",
    "focus_hash": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
    "issued_at": "2026-03-20T14:30:35.200Z",
    "expires_at": "2026-03-20T14:31:05.200Z",
    "nonce": "c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9",
    "signature": "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2"
  }
}
```

### Error Response (400) -- Unknown candidate

```json
{
  "error": "BadRequest",
  "message": "Unknown candidate_id 'elem-999'"
}
```

### Error Response (409) -- Focus drift

```json
{
  "error": "Conflict",
  "message": "Focused window changed"
}
```

---

## 5. POST /api/automation/gui/sessions/{id}/execute

Execute an action using a signed ticket.

### Request

```bash
curl -s -X POST http://localhost:10090/api/automation/gui/sessions/a1b2c3d4-e5f6-7890-abcd-ef1234567890/execute \
  -H "Content-Type: application/json" \
  -H "x-gui-session-token: b7f3a1d9c8e2f4a6b0d1e3f5a7c9d2e4f6a8b0c2d4e6f8a0b2c4d6e8f0a2b4" \
  -d '{
    "ticket": {
      "schema_version": "automation.gui.ticket.v1",
      "ticket_id": "f1e2d3c4-b5a6-9870-fedc-ba0987654321",
      "session_id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
      "scene_id": "scene-20260320-143022-001",
      "element_id": "elem-001",
      "action_hash": "a9f8e7d6c5b4a3f2e1d0c9b8a7f6e5d4c3b2a1f0e9d8c7b6a5f4e3d2c1b0a9",
      "focus_hash": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
      "issued_at": "2026-03-20T14:30:35.200Z",
      "expires_at": "2026-03-20T14:31:05.200Z",
      "nonce": "c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9",
      "signature": "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2"
    }
  }'
```

### Success Response (200)

```json
{
  "schema_version": "automation.gui.v2",
  "command_id": "gui-action-1710942635200",
  "ticket": {
    "schema_version": "automation.gui.ticket.v1",
    "ticket_id": "f1e2d3c4-b5a6-9870-fedc-ba0987654321",
    "session_id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
    "scene_id": "scene-20260320-143022-001",
    "element_id": "elem-001",
    "action_hash": "a9f8e7d6c5b4a3f2e1d0c9b8a7f6e5d4c3b2a1f0e9d8c7b6a5f4e3d2c1b0a9",
    "focus_hash": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
    "issued_at": "2026-03-20T14:30:35.200Z",
    "expires_at": "2026-03-20T14:31:05.200Z",
    "nonce": "c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9",
    "signature": "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c2d4e6f8a0b2c4d6e8f0a2b4"
  },
  "result": {
    "success": true,
    "element": {
      "text": "Save File",
      "bounds": { "x": 120, "y": 45, "width": 80, "height": 32 },
      "role": "button",
      "confidence": 0.92,
      "source": "Ocr"
    },
    "verification": {
      "screen_changed": true,
      "changed_regions": 1
    },
    "retry_count": 0,
    "elapsed_ms": 145,
    "error": null
  },
  "outcome": {
    "session": {
      "session_id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
      "state": "executed",
      "...remaining session fields..."
    },
    "succeeded": true,
    "detail": null
  }
}
```

### Error Response (409) -- Focus drift

```json
{
  "error": "Conflict",
  "message": "Focused window changed"
}
```

### Error Response (422) -- Ticket expired

```json
{
  "error": "Unprocessable",
  "message": "ticket expired"
}
```

### Error Response (422) -- Nonce replay

```json
{
  "error": "Unprocessable",
  "message": "ticket nonce replay detected"
}
```

---

## 6. DELETE /api/automation/gui/sessions/{id}

Cancel and delete a session.

### Request

```bash
curl -s -X DELETE http://localhost:10090/api/automation/gui/sessions/a1b2c3d4-e5f6-7890-abcd-ef1234567890 \
  -H "x-gui-session-token: b7f3a1d9c8e2f4a6b0d1e3f5a7c9d2e4f6a8b0c2d4e6f8a0b2c4d6e8f0a2b4"
```

### Success Response (200)

```json
{
  "schema_version": "automation.gui.v2",
  "session": {
    "session_id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
    "state": "cancelled",
    "updated_at": "2026-03-20T14:32:00.100Z",
    "...remaining session fields..."
  }
}
```

### Error Response (404) -- Session not found

```json
{
  "error": "NotFound",
  "message": "a1b2c3d4-e5f6-7890-abcd-ef1234567890"
}
```

---

## 7. GET /api/automation/gui/sessions/{id}/events

Subscribe to SSE event stream for a session.

### Request

```bash
curl -s -N http://localhost:10090/api/automation/gui/sessions/a1b2c3d4-e5f6-7890-abcd-ef1234567890/events \
  -H "x-gui-session-token: b7f3a1d9c8e2f4a6b0d1e3f5a7c9d2e4f6a8b0c2d4e6f8a0b2c4d6e8f0a2b4" \
  -H "Accept: text/event-stream"
```

### SSE Event Stream

```
event: proposed
data: {"schema_version":"automation.gui.event.v1","event_type":"gui_session.proposed","session_id":"a1b2c3d4-e5f6-7890-abcd-ef1234567890","state":"proposed","emitted_at":"2026-03-20T14:30:22.500Z","message":null}

event: highlighted
data: {"schema_version":"automation.gui.event.v1","event_type":"gui_session.highlighted","session_id":"a1b2c3d4-e5f6-7890-abcd-ef1234567890","state":"highlighted","emitted_at":"2026-03-20T14:30:28.100Z","message":null}

event: confirmed
data: {"schema_version":"automation.gui.event.v1","event_type":"gui_session.confirmed","session_id":"a1b2c3d4-e5f6-7890-abcd-ef1234567890","state":"confirmed","emitted_at":"2026-03-20T14:30:35.200Z","message":"candidate_id=elem-001"}

event: executing
data: {"schema_version":"automation.gui.event.v1","event_type":"gui_session.executing","session_id":"a1b2c3d4-e5f6-7890-abcd-ef1234567890","state":"executing","emitted_at":"2026-03-20T14:30:36.000Z","message":"ticket_id=f1e2d3c4-b5a6-9870-fedc-ba0987654321"}

event: executed
data: {"schema_version":"automation.gui.event.v1","event_type":"gui_session.executed","session_id":"a1b2c3d4-e5f6-7890-abcd-ef1234567890","state":"executed","emitted_at":"2026-03-20T14:30:36.200Z","message":null}

: keep-alive

```

Keep-alive pings are sent every 15 seconds as SSE comments (`: keep-alive`).

### Error Response (401) -- Missing or invalid token

```json
{
  "error": "Unauthorized",
  "message": "Unauthorized"
}
```

---

## Error Code Reference

| HTTP | ApiError Variant | Domain Error | When |
|------|-----------------|-------------|------|
| 400 | `BadRequest` | `BadRequest(msg)` | Invalid input, unknown candidate, no targets |
| 401 | `Unauthorized` | `Unauthorized` | Missing or invalid `x-gui-session-token` |
| 403 | `Forbidden` | `Forbidden(msg)` | OS denied accessibility access |
| 404 | `NotFound` | `NotFound(msg)` | Session ID not found |
| 409 | `Conflict` | `FocusDrift(msg)` | Window focus changed since session creation |
| 422 | `Unprocessable` | `TicketInvalid(msg)` | Expired ticket, nonce replay, signature mismatch |
| 500 | `Internal` | `Internal(msg)` | Unexpected server error |
| 503 | `ServiceUnavailable` | `Unavailable(msg)` | HMAC secret missing, GUI disabled |
