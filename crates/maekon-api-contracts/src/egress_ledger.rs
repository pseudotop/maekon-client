//! Egress transparency browser API contracts (T1.2, #7910).
//!
//! Read-only DTOs for `GET /api/privacy/egress-ledger` — the local "Trust
//! Console" panel that answers *what left this device*. Backed by the #4803
//! egress audit ledger via the read-only
//! [`EgressLedgerReaderPort`](maekon_core::ports::egress_ledger_reader). The
//! ledger is erase-retained regulatory-compliance evidence, so these contracts
//! expose **read only** — there is no create/update/delete DTO by design.
//!
//! The nine surfaced fields are the ledger's own columns, which contain no
//! captured content / PII by construction (they record *that* egress happened —
//! byte counts, destination sink strings, disposition — never *what* was sent).

use serde::{Deserialize, Serialize};

/// Query parameters for `GET /api/privacy/egress-ledger`.
///
/// When both `from` and `to` (inclusive RFC3339 timestamps) are present the
/// endpoint returns the `[from, to]` range; otherwise it returns the most
/// recent entries capped at `limit`. `limit` is defaulted and clamped by the
/// handler (DoS guard).
#[derive(Debug, Default, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct EgressLedgerQuery {
    /// Maximum number of entries to return in recent mode (default: 100, capped
    /// at 1000). Ignored when a `from`/`to` range is supplied.
    #[serde(default)]
    pub limit: Option<usize>,
    /// Inclusive range start (RFC3339). Requires `to` to take effect.
    #[serde(default)]
    pub from: Option<String>,
    /// Inclusive range end (RFC3339). Requires `from` to take effect.
    #[serde(default)]
    pub to: Option<String>,
}

/// One egress-ledger row — the audit record of a single egress (or prevented
/// capture) event. Field-for-field the ledger's own nine columns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct EgressLedgerEntryDto {
    /// Caller-generated UUID (`egress_ledger.record_id`, UNIQUE).
    pub record_id: String,
    /// Producing event type (e.g. `Context`, `CrossDeviceSync`, `DeletionEvent`).
    pub event_type: String,
    /// Associated event id, when known.
    pub event_id: Option<String>,
    /// Serialized plaintext payload size in bytes. `0` for a `capture_blocked`
    /// row (a frame that was deliberately NOT captured — no bytes were sent).
    pub byte_count: i64,
    /// Number of recipients this egress was delivered to. `0` for a
    /// `capture_blocked` row (nothing left the device).
    pub recipient_count: i64,
    /// Egress destination / sink target string (e.g. `server.batch_upload`,
    /// `sync.lan`, or `local.capture` for a prevented capture).
    pub destination: String,
    /// Disposition: `uploaded`, `blocked`, or `capture_blocked` (a frame
    /// excluded before capture — prevented-capture evidence, not egress).
    pub disposition: String,
    /// Consent snapshot at the egress moment.
    pub consent_state: String,
    /// Occurrence timestamp (RFC3339).
    pub occurred_at: String,
}

impl From<maekon_core::models::storage_records::EgressLedgerRecord> for EgressLedgerEntryDto {
    fn from(record: maekon_core::models::storage_records::EgressLedgerRecord) -> Self {
        Self {
            record_id: record.record_id,
            event_type: record.event_type,
            event_id: record.event_id,
            byte_count: record.byte_count,
            recipient_count: record.recipient_count,
            destination: record.destination,
            disposition: record.disposition,
            consent_state: record.consent_state,
            occurred_at: record.occurred_at,
        }
    }
}

/// Response body for `GET /api/privacy/egress-ledger`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct EgressLedgerResponse {
    /// The matched ledger rows. Recent mode: newest-first. Range mode:
    /// oldest-first (matching the underlying reader ordering).
    pub entries: Vec<EgressLedgerEntryDto>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use maekon_core::models::storage_records::EgressLedgerRecord;

    fn sample_record(
        disposition: &str,
        byte_count: i64,
        recipient_count: i64,
    ) -> EgressLedgerRecord {
        EgressLedgerRecord {
            record_id: "rec-1".to_string(),
            event_type: "Context".to_string(),
            event_id: Some("evt-1".to_string()),
            byte_count,
            recipient_count,
            destination: "server.batch_upload".to_string(),
            disposition: disposition.to_string(),
            consent_state: "telemetry=true".to_string(),
            occurred_at: "2026-07-07T12:00:00Z".to_string(),
        }
    }

    #[test]
    fn dto_maps_all_nine_columns_from_record() {
        let dto = EgressLedgerEntryDto::from(sample_record("uploaded", 128, 1));
        assert_eq!(dto.record_id, "rec-1");
        assert_eq!(dto.event_type, "Context");
        assert_eq!(dto.event_id.as_deref(), Some("evt-1"));
        assert_eq!(dto.byte_count, 128);
        assert_eq!(dto.recipient_count, 1);
        assert_eq!(dto.destination, "server.batch_upload");
        assert_eq!(dto.disposition, "uploaded");
        assert_eq!(dto.consent_state, "telemetry=true");
        assert_eq!(dto.occurred_at, "2026-07-07T12:00:00Z");
    }

    #[test]
    fn capture_blocked_row_round_trips_with_zero_bytes() {
        // A prevented-capture row: byte_count/recipient_count = 0, a distinct
        // destination + disposition the panel renders as "capture blocked".
        let mut record = sample_record("capture_blocked", 0, 0);
        record.destination = "local.capture".to_string();
        let response = EgressLedgerResponse {
            entries: vec![EgressLedgerEntryDto::from(record)],
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"disposition\":\"capture_blocked\""));
        assert!(json.contains("\"destination\":\"local.capture\""));
        assert!(json.contains("\"byte_count\":0"));
        assert!(json.contains("\"recipient_count\":0"));
    }
}
