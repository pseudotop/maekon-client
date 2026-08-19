//! `GET /api/audit/verify` — durable audit-log hash-chain integrity verification (#7600).
//!
//! Before this route existed, `verify_audit_chain()` (the real ADR-072
//! SHA-256 hash-chain check) had zero webview callers: only a Tauri IPC
//! command (`verify_audit_log` in `src-tauri`) reached it, and that command
//! had no frontend caller either. The compliance capability was advertised
//! (implemented, tested) but unreachable from the shipped UI. This handler
//! mirrors `GET /api/audit/export`'s wiring so both the desktop webview and
//! the standalone browser surface (`IS_TAURI=false`) can trigger a
//! verification and see the pass/fail + first-broken-entry result.

use axum::extract::State;
use axum::Json;

use maekon_core::models::audit::AuditChainReport;

use crate::error::ApiError;
use crate::AppState;

/// `GET /api/audit/verify` handler.
///
/// # Errors
/// - `503 Service Unavailable`: when `core.audit_chain_verifier` is None
///   (standalone web server built without a durable SQLite storage backing).
pub async fn verify_audit(
    State(state): State<AppState>,
) -> Result<Json<AuditChainReport>, ApiError> {
    let Some(verifier) = state.core.audit_chain_verifier.as_ref() else {
        return Err(ApiError::ServiceUnavailable(
            "audit chain verifier not configured".into(),
        ));
    };
    let report = verifier.verify_audit_chain().await;
    Ok(Json(report))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use axum::extract::State;
    use maekon_core::models::audit::{AuditChainBreak, AuditChainReport};
    use maekon_core::ports::audit_chain_verifier::AuditChainVerifierPort;
    use maekon_storage::sqlite::SqliteStorage;

    use super::*;

    /// Test-only `AuditChainVerifierPort` implementation returning a
    /// pre-seeded report, so the handler's pass-through behavior can be
    /// asserted without depending on real SQLite hash-chain internals here
    /// (those are covered directly in `maekon-storage`).
    struct StubVerifier(AuditChainReport);

    #[async_trait]
    impl AuditChainVerifierPort for StubVerifier {
        async fn verify_audit_chain(&self) -> AuditChainReport {
            self.0.clone()
        }
    }

    fn fixture_state_no_verifier() -> AppState {
        crate::test_local_auth::test_app_state()
    }

    fn fixture_state_with_verifier(verifier: Arc<dyn AuditChainVerifierPort>) -> AppState {
        let mut state = fixture_state_no_verifier();
        state.core.audit_chain_verifier = Some(verifier);
        state
    }

    /// Test 1: returns 503 when no verifier is wired (mirrors
    /// `audit_export_returns_503_when_logger_none`).
    #[tokio::test]
    async fn verify_audit_returns_503_when_verifier_not_configured() {
        let state = fixture_state_no_verifier();

        let err = verify_audit(State(state)).await.unwrap_err();

        assert!(
            matches!(err, ApiError::ServiceUnavailable(_)),
            "expected ServiceUnavailable, got {err:?}"
        );
    }

    /// Test 2: passes through an OK report unchanged.
    #[tokio::test]
    async fn verify_audit_returns_ok_report_from_verifier() {
        let report = AuditChainReport {
            ok: true,
            first_seq: Some(0),
            last_seq: Some(4),
            verified_count: 5,
            legacy_unchained_count: 0,
            first_break: None,
        };
        let state = fixture_state_with_verifier(Arc::new(StubVerifier(report.clone())));

        let Json(body) = verify_audit(State(state))
            .await
            .expect("handler should succeed");

        assert_eq!(body, report);
    }

    /// Test 3: passes through a broken-chain report with the
    /// first-broken-entry detail intact (the field the frontend affordance
    /// renders).
    #[tokio::test]
    async fn verify_audit_returns_broken_report_with_first_break_detail() {
        let report = AuditChainReport {
            ok: false,
            first_seq: Some(0),
            last_seq: Some(4),
            verified_count: 2,
            legacy_unchained_count: 0,
            first_break: Some(AuditChainBreak {
                seq: 2,
                reason: "entry_hash mismatch (row tampered)".to_string(),
            }),
        };
        let state = fixture_state_with_verifier(Arc::new(StubVerifier(report.clone())));

        let Json(body) = verify_audit(State(state))
            .await
            .expect("handler should succeed");

        assert!(!body.ok);
        assert_eq!(body.first_break.as_ref().map(|b| b.seq), Some(2));
        assert_eq!(
            body.first_break.as_ref().map(|b| b.reason.as_str()),
            Some("entry_hash mismatch (row tampered)")
        );
    }

    /// Test 4: end-to-end against the REAL `SqliteStorage` port impl (not a
    /// stub) — a genuine regression guard that the handler is wired to the
    /// actual ADR-072 verification, not just a compatible interface.
    #[tokio::test]
    async fn verify_audit_reaches_real_sqlite_verification() {
        let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("in-memory sqlite"));
        storage.save_audit_entry(&maekon_core::models::audit::AuditEntry {
            entry_id: "aud-real-1".to_string(),
            timestamp: chrono::Utc::now(),
            session_id: "sess-real".to_string(),
            command_id: "cmd-real".to_string(),
            action_type: "MouseClick".to_string(),
            status: maekon_core::models::audit::AuditStatus::Completed,
            details: Some("real chain entry".to_string()),
            execution_time_ms: Some(5),
        });
        let verifier: Arc<dyn AuditChainVerifierPort> = storage;
        let state = fixture_state_with_verifier(verifier);

        let Json(body) = verify_audit(State(state))
            .await
            .expect("handler should succeed");

        assert!(
            body.ok,
            "real single-entry chain should verify ok: {body:?}"
        );
        assert_eq!(body.verified_count, 1);
    }
}
