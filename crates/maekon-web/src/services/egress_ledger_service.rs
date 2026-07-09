//! Egress transparency browser query service (T1.2, #7910).
//!
//! Pure orchestration over the read-only
//! [`EgressLedgerReaderPort`](maekon_core::ports::egress_ledger_reader): clamps
//! the limit (DoS guard), selects recent-vs-range mode, and maps the
//! erase-retained #4803 ledger rows into the transport
//! [`EgressLedgerResponse`] DTO. No mutation surface — the ledger is
//! compliance evidence, read only by design.

use std::sync::Arc;

use maekon_api_contracts::egress_ledger::{
    EgressLedgerEntryDto, EgressLedgerQuery, EgressLedgerResponse,
};
use maekon_core::ports::egress_ledger_reader::EgressLedgerReaderPort;

/// Default number of recent entries returned when `limit` is absent.
const DEFAULT_LIMIT: usize = 100;

/// DoS guard: maximum entries returned in recent mode (mirrors the audit-export
/// sibling's 1000 cap).
const MAX_LIMIT: usize = 1000;

/// Build the egress-ledger response for a `GET /api/privacy/egress-ledger`
/// request.
///
/// Range mode: when BOTH `from` and `to` are present and non-empty, returns the
/// inclusive `[from, to]` RFC3339 range (oldest-first). Recent mode otherwise:
/// the newest entries capped at `limit` (default 100, clamped to 1000).
pub fn build_egress_ledger_response(
    reader: &Arc<dyn EgressLedgerReaderPort>,
    query: &EgressLedgerQuery,
) -> EgressLedgerResponse {
    let records = match (query.from.as_deref(), query.to.as_deref()) {
        (Some(from), Some(to)) if !from.is_empty() && !to.is_empty() => {
            reader.egress_between(from, to)
        }
        _ => {
            let limit = query.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT);
            reader.recent_egress(limit)
        }
    };

    EgressLedgerResponse {
        entries: records
            .into_iter()
            .map(EgressLedgerEntryDto::from)
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maekon_core::models::storage_records::EgressLedgerRecord;
    use maekon_storage::sqlite::SqliteStorage;

    fn record(record_id: &str, disposition: &str, occurred_at: &str) -> EgressLedgerRecord {
        EgressLedgerRecord {
            record_id: record_id.to_string(),
            event_type: "Context".to_string(),
            event_id: Some(format!("evt-{record_id}")),
            byte_count: 128,
            recipient_count: 1,
            destination: "server.batch_upload".to_string(),
            disposition: disposition.to_string(),
            consent_state: "telemetry=true".to_string(),
            occurred_at: occurred_at.to_string(),
        }
    }

    /// Seeds a real in-memory `SqliteStorage` with three egress rows across two
    /// days, returning it as the read-only reader port (Port Instance Sharing).
    fn seeded_reader() -> Arc<dyn EgressLedgerReaderPort> {
        let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("in-memory sqlite"));
        storage
            .record_egress(&record("r1", "uploaded", "2026-07-05T09:00:00Z"))
            .expect("write r1");
        storage
            .record_egress(&record("r2", "blocked", "2026-07-06T09:00:00Z"))
            .expect("write r2");
        storage
            .record_egress(&record("r3", "uploaded", "2026-07-07T09:00:00Z"))
            .expect("write r3");
        storage as Arc<dyn EgressLedgerReaderPort>
    }

    #[test]
    fn recent_mode_returns_all_newest_first_by_default() {
        let reader = seeded_reader();
        let query = EgressLedgerQuery::default();

        let response = build_egress_ledger_response(&reader, &query);

        assert_eq!(response.entries.len(), 3);
        // recent_egress orders occurred_at DESC.
        assert_eq!(response.entries[0].record_id, "r3");
        assert_eq!(response.entries[2].record_id, "r1");
    }

    #[test]
    fn recent_mode_honors_limit() {
        let reader = seeded_reader();
        let query = EgressLedgerQuery {
            limit: Some(1),
            from: None,
            to: None,
        };

        let response = build_egress_ledger_response(&reader, &query);

        assert_eq!(response.entries.len(), 1);
        assert_eq!(response.entries[0].record_id, "r3");
    }

    #[test]
    fn range_mode_filters_between_inclusive_oldest_first() {
        let reader = seeded_reader();
        let query = EgressLedgerQuery {
            limit: None,
            from: Some("2026-07-05T00:00:00Z".to_string()),
            to: Some("2026-07-06T23:59:59Z".to_string()),
        };

        let response = build_egress_ledger_response(&reader, &query);

        // r1 + r2 fall in range; r3 (07-07) excluded. egress_between orders ASC.
        assert_eq!(response.entries.len(), 2);
        assert_eq!(response.entries[0].record_id, "r1");
        assert_eq!(response.entries[1].record_id, "r2");
    }

    #[test]
    fn partial_range_falls_back_to_recent_mode() {
        let reader = seeded_reader();
        // Only `from` present → not a valid range → recent mode (all 3).
        let query = EgressLedgerQuery {
            limit: None,
            from: Some("2026-07-05T00:00:00Z".to_string()),
            to: None,
        };

        let response = build_egress_ledger_response(&reader, &query);

        assert_eq!(response.entries.len(), 3);
    }
}
