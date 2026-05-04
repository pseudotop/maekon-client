# ONESHIM Integration CloudEvents Profile

Status: Draft  
Domain: `integration`  
CloudEvents baseline: `1.0`

## Purpose

Define how ONESHIM integration domain messages map onto CloudEvents for outbound sync and inbound prompt delivery.

This profile is designed to stay friendly to:

- CloudEvents routing
- CESQL filtering
- transport-neutral session adapters

## Structured Event Encoding

Preferred encoding:

- `application/cloudevents+json`

Each integration event is sent as one CloudEvent.

Batching may happen at the transport layer, but the event envelope stays one-event-per-message at the domain boundary.

## Core Mapping

| ONESHIM field | CloudEvents field | Notes |
| --- | --- | --- |
| `IntegrationEnvelope.envelope_id` | `id` | Stable event id |
| `IntegrationEnvelope.timestamp` | `time` | RFC 3339 timestamp |
| `IntegrationEnvelope.message_type` | `type` | Mapped to stable event type names |
| `IntegrationOrigin.device_id` | `source` | Use device-scoped URI-like source |
| payload schema version | `dataschema` | Stable schema reference when published |
| domain payload | `data` | Packet/prompt/ack/session body |

## Event Type Mapping

Recommended event types:

- `io.oneshim.integration.insight.v1`
- `io.oneshim.integration.prompt.v1`
- `io.oneshim.integration.prompt_receipt.v1`
- `io.oneshim.integration.session.v1`
- `io.oneshim.integration.ack.v1`
- `io.oneshim.integration.audit.v1`

## Source Mapping

Recommended source shape:

- `oneshim://devices/{device_id}`

If workspace context is relevant, keep it in an extension attribute rather than encoding it into multiple incompatible source formats.

## Subject Mapping

Recommended `subject` values:

- insight packet: `{packet_id}`
- proactive prompt: `{prompt_id}`
- prompt receipt: `{prompt_id}`
- session state: `session/{session_id}`
- ack event: `cursor/{stream_id}`

## ONESHIM Extension Attributes

Use lower-case ASCII extension names so CESQL and cross-platform routing stay simple.

Recommended extensions:

- `oneshimscope`
  - integration capability scope
- `oneshimnonce`
  - replay/uniqueness support
- `oneshimsessionid`
  - optional active session id
- `oneshimworkspaceid`
  - optional workspace or tenant id
- `oneshimprivacy`
  - privacy classification for outbound insight/audit events
- `oneshimpromptcategory`
  - prompt category for inbound proactive prompt events
- `oneshimqueueid`
  - client outbox queue id for delivery-level acknowledgements on live transports

## CESQL-Friendly Rules

To keep routing/filtering straightforward:

- keep extension names stable and flat
- avoid nested routing-critical metadata in `data`
- prefer small enumerated strings for scope, privacy, and prompt category
- keep `type` versioned and explicit

Examples of useful filters:

- `type = "io.oneshim.integration.insight.v1"`
- `oneshimscope = "insight:write"`
- `oneshimprivacy = "derived_summary"`
- `type = "io.oneshim.integration.prompt.v1" AND oneshimpromptcategory = "task"`
- `type = "io.oneshim.integration.prompt_receipt.v1" AND oneshimscope = "prompt:ack"`

## Privacy Guidance

CloudEvents metadata must not be used to leak raw sensitive source data.

Allowed in metadata:

- scope
- privacy class
- session id
- workspace id
- category

Not allowed in metadata:

- raw OCR text
- raw screenshot content
- raw window/document contents
- raw user-entered secrets

Sensitive content belongs in `data` only when policy and consent rules allow the payload itself.

## Example: Outbound Insight Event

```json
{
  "specversion": "1.0",
  "id": "env-001",
  "source": "oneshim://device/device-001",
  "type": "io.oneshim.integration.insight.v1",
  "subject": "packet-001",
  "time": "2026-03-16T10:20:30Z",
  "datacontenttype": "application/json",
  "dataschema": "integration.envelope.v1",
  "oneshimscope": "insight:write",
  "oneshimnonce": "nonce-001",
  "oneshimsessionid": "session-001",
  "oneshimworkspaceid": "workspace-001",
  "oneshimqueueid": "queue-001",
  "oneshimprivacy": "derived_summary",
  "data": {
    "packet_id": "packet-001",
    "summary": "User spent 25 minutes in focused editing mode.",
    "derived_tags": ["focus", "editing"]
  }
}
```

## Example: Inbound Prompt Event

```json
{
  "specversion": "1.0",
  "id": "env-002",
  "source": "oneshim://device/device-001",
  "type": "io.oneshim.integration.prompt.v1",
  "subject": "prompt-001",
  "time": "2026-03-16T10:25:00Z",
  "datacontenttype": "application/json",
  "dataschema": "integration.envelope.v1",
  "oneshimscope": "prompt:read",
  "oneshimnonce": "nonce-002",
  "oneshimsessionid": "session-001",
  "oneshimworkspaceid": "workspace-001",
  "oneshimpromptcategory": "task",
  "data": {
    "prompt_id": "prompt-001",
    "title": "Review the build failure",
    "body": "A teammate requested a quick triage on the failing pipeline."
  }
}
```

## Example: Outbound Prompt Receipt Event

```json
{
  "specversion": "1.0",
  "id": "env-003",
  "source": "oneshim://devices/device-001",
  "type": "io.oneshim.integration.prompt_receipt.v1",
  "subject": "prompt-001",
  "time": "2026-03-16T10:27:00Z",
  "datacontenttype": "application/json",
  "dataschema": "integration.prompt_receipt.v1",
  "oneshimscope": "prompt:ack",
  "oneshimnonce": "nonce-003",
  "oneshimsessionid": "session-001",
  "oneshimqueueid": "queue-003",
  "data": {
    "receipt_id": "receipt-001",
    "prompt_id": "prompt-001",
    "action": "dismissed",
    "reason": "handled locally"
  }
}
```

## Review Notes

- This profile intentionally keeps MCP and A2A out of the envelope definition.
- Those are adapter-level standards and should map into this domain instead of replacing it.
- If transport adapters need protocol-specific metadata, keep that outside the CloudEvents envelope whenever possible.
