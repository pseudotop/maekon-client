//! `GET /api/privacy/egress-ledger` — egress transparency browser (T1.2, #7910).
//!
//! Read-only "what left this device" audit panel backing the local Trust
//! Console. Serves the erase-retained #4803 egress ledger through the read-only
//! [`EgressLedgerReaderPort`](maekon_core::ports::egress_ledger_reader), which
//! is the SAME concrete `SqliteStorage` the scheduler's `EgressLedgerSink`
//! writer records into (Port Instance Sharing). Before this route the ledger
//! readers (`recent_egress` / `egress_between`) had ZERO non-test callers — the
//! egress audit evidence existed but had no delivery path to the UI.
//!
//! Deliberately GET-only: the ledger is regulatory-compliance evidence retained
//! across GDPR Art. 17 erasure, so there is no delete/edit affordance anywhere
//! in this surface.

use axum::extract::{Query, State};
use axum::Json;

use maekon_api_contracts::egress_ledger::{EgressLedgerQuery, EgressLedgerResponse};

use crate::error::ApiError;
use crate::services::egress_ledger_service::build_egress_ledger_response;
use crate::AppState;

/// `GET /api/privacy/egress-ledger` handler.
///
/// Query params: `limit` (recent mode, default 100, capped 1000) and optional
/// `from` + `to` (inclusive RFC3339 range mode when both present).
///
/// # Errors
/// - `503 Service Unavailable`: when `core.egress_ledger_reader` is None
///   (standalone web-server build without a durable SQLite storage backing).
pub async fn get_egress_ledger(
    State(state): State<AppState>,
    Query(query): Query<EgressLedgerQuery>,
) -> Result<Json<EgressLedgerResponse>, ApiError> {
    let Some(reader) = state.core.egress_ledger_reader.as_ref() else {
        return Err(ApiError::ServiceUnavailable(
            "egress ledger reader not configured".into(),
        ));
    };
    Ok(Json(build_egress_ledger_response(reader, &query)))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use maekon_core::models::storage_records::EgressLedgerRecord;
    use maekon_core::ports::egress_ledger_reader::EgressLedgerReaderPort;
    use maekon_storage::sqlite::SqliteStorage;

    use super::*;

    fn record(
        record_id: &str,
        disposition: &str,
        destination: &str,
        byte_count: i64,
        recipient_count: i64,
        occurred_at: &str,
    ) -> EgressLedgerRecord {
        EgressLedgerRecord {
            record_id: record_id.to_string(),
            event_type: "Context".to_string(),
            event_id: Some(format!("evt-{record_id}")),
            byte_count,
            recipient_count,
            destination: destination.to_string(),
            disposition: disposition.to_string(),
            consent_state: "telemetry=true".to_string(),
            occurred_at: occurred_at.to_string(),
        }
    }

    /// Real `SqliteStorage` seeded with a normal upload, a blocked egress, and a
    /// `capture_blocked` (prevented-capture) row; returned as `AppState` with
    /// the reader wired.
    fn state_with_seeded_reader() -> AppState {
        let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("in-memory sqlite"));
        storage
            .record_egress(&record(
                "up-1",
                "uploaded",
                "server.batch_upload",
                256,
                1,
                "2026-07-05T09:00:00Z",
            ))
            .expect("write up-1");
        storage
            .record_egress(&record(
                "bl-1",
                "blocked",
                "server.batch_upload",
                0,
                0,
                "2026-07-06T09:00:00Z",
            ))
            .expect("write bl-1");
        // T1.1 (#7922) capture-block row: byte/recipient 0, local.capture sink.
        storage
            .record_egress(&record(
                "cap-1",
                "capture_blocked",
                "local.capture",
                0,
                0,
                "2026-07-07T09:00:00Z",
            ))
            .expect("write cap-1");

        let mut state = crate::test_local_auth::test_app_state();
        state.core.egress_ledger_reader = Some(storage as Arc<dyn EgressLedgerReaderPort>);
        state
    }

    /// Test 1: returns 503 when no reader is wired (mirrors
    /// `audit_verify_returns_503_when_verifier_not_configured`).
    #[tokio::test]
    async fn returns_503_when_reader_not_configured() {
        let state = crate::test_local_auth::test_app_state();

        let err = get_egress_ledger(State(state), Query(EgressLedgerQuery::default()))
            .await
            .unwrap_err();

        assert!(
            matches!(err, ApiError::ServiceUnavailable(_)),
            "expected ServiceUnavailable, got {err:?}"
        );
    }

    /// Test 2: recent mode returns every seeded row newest-first over the REAL
    /// SqliteStorage reader (regression guard the endpoint reaches the actual
    /// #4803 ledger, not a compatible stub).
    #[tokio::test]
    async fn recent_mode_returns_all_rows_newest_first() {
        let state = state_with_seeded_reader();

        let Json(body) = get_egress_ledger(State(state), Query(EgressLedgerQuery::default()))
            .await
            .expect("handler should succeed");

        assert_eq!(body.entries.len(), 3);
        assert_eq!(body.entries[0].record_id, "cap-1");
        assert_eq!(body.entries[2].record_id, "up-1");
    }

    /// Test 3: range mode filters `[from, to]` inclusive (oldest-first).
    #[tokio::test]
    async fn range_mode_filters_between() {
        let state = state_with_seeded_reader();
        let query = EgressLedgerQuery {
            limit: None,
            from: Some("2026-07-05T00:00:00Z".to_string()),
            to: Some("2026-07-06T23:59:59Z".to_string()),
        };

        let Json(body) = get_egress_ledger(State(state), Query(query))
            .await
            .expect("handler should succeed");

        assert_eq!(body.entries.len(), 2);
        assert_eq!(body.entries[0].record_id, "up-1");
        assert_eq!(body.entries[1].record_id, "bl-1");
    }

    /// Test 4: a `capture_blocked` row round-trips through the endpoint DTO with
    /// its distinct disposition/destination and zero byte/recipient counts
    /// intact — the fields the panel needs to render it as "capture blocked"
    /// (prevented-capture evidence), not "0 bytes uploaded".
    #[tokio::test]
    async fn capture_blocked_row_round_trips_through_endpoint() {
        let state = state_with_seeded_reader();

        let Json(body) = get_egress_ledger(State(state), Query(EgressLedgerQuery::default()))
            .await
            .expect("handler should succeed");

        let cap = body
            .entries
            .iter()
            .find(|e| e.record_id == "cap-1")
            .expect("capture_blocked row present");
        assert_eq!(cap.disposition, "capture_blocked");
        assert_eq!(cap.destination, "local.capture");
        assert_eq!(cap.byte_count, 0);
        assert_eq!(cap.recipient_count, 0);
    }
}
