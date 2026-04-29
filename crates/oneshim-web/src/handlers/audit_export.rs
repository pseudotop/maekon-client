//! `GET /api/audit/export` — audit entry export with optional filtering.
//!
//! Spec §5.9 / D25 / NV1. Supports `command_id` + `status` (reserved) +
//! `limit` query params. Response: `Vec<AuditEntry>` newest-first.
//! DoS cap: limit clamped to 1000.
//!
//! 503 when `automation.audit_logger` is None (audit logger not configured).

use axum::extract::{Query, State};
use axum::Json;

use oneshim_api_contracts::audit_export::AuditExportQuery;
use oneshim_core::models::audit::AuditEntry;

use crate::error::ApiError;
use crate::AppState;

/// `GET /api/audit/export` 핸들러
///
/// `command_id` 쿼리 파라미터가 있으면 해당 command_id로 필터링한 엔트리를 반환합니다.
/// 없으면 최근 엔트리를 반환합니다. `limit` 파라미터는 최대 1000개로 제한됩니다.
/// `status` 파라미터는 예약됨 (현재 no-op).
///
/// # Errors
/// - `503 Service Unavailable`: `automation.audit_logger`가 None인 경우
pub async fn export_audit(
    State(state): State<AppState>,
    Query(query): Query<AuditExportQuery>,
) -> Result<Json<Vec<AuditEntry>>, ApiError> {
    let limit = query.limit.unwrap_or(100).min(1000);
    let Some(audit_logger) = state.automation.audit_logger.as_ref() else {
        return Err(ApiError::ServiceUnavailable(
            "audit logger not configured".into(),
        ));
    };
    let entries = match &query.command_id {
        Some(cmd_id) if !cmd_id.is_empty() => {
            audit_logger.entries_by_command_id(cmd_id, limit).await
        }
        _ => audit_logger.recent_entries(limit).await,
    };
    Ok(Json(entries))
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use chrono::Utc;
    use oneshim_core::models::ai_session::SessionAuditEntry;
    use oneshim_core::models::audit::{AuditLevel, AuditStats, AuditStatus};
    use oneshim_core::ports::audit_log::AuditLogPort;
    use oneshim_storage::sqlite::SqliteStorage;
    use std::sync::{Arc, Mutex};
    use tokio::sync::broadcast;
    use uuid::Uuid;

    /// 테스트용 AuditLogPort 구현 — 시드된 항목으로 recent_entries 및
    /// entries_by_command_id 쿼리를 지원합니다.
    struct SeedableAuditLog {
        entries: Mutex<Vec<AuditEntry>>,
    }

    impl SeedableAuditLog {
        /// 주어진 항목들로 초기화된 새 인스턴스를 생성합니다.
        fn with_entries(entries: Vec<AuditEntry>) -> Arc<Self> {
            Arc::new(Self {
                entries: Mutex::new(entries),
            })
        }

        fn empty() -> Arc<Self> {
            Self::with_entries(vec![])
        }
    }

    /// 고정된 command_id와 action_type으로 AuditEntry를 생성하는 헬퍼
    fn make_entry(command_id: &str, action_type: &str) -> AuditEntry {
        AuditEntry {
            entry_id: Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            session_id: "sess-test".to_string(),
            command_id: command_id.to_string(),
            action_type: action_type.to_string(),
            status: AuditStatus::Completed,
            details: None,
            execution_time_ms: Some(10),
        }
    }

    #[async_trait]
    impl AuditLogPort for SeedableAuditLog {
        async fn pending_count(&self) -> usize {
            0
        }

        async fn recent_entries(&self, limit: usize) -> Vec<AuditEntry> {
            self.entries
                .lock()
                .unwrap()
                .iter()
                .take(limit)
                .cloned()
                .collect()
        }

        async fn entries_by_status(&self, _status: &AuditStatus, _limit: usize) -> Vec<AuditEntry> {
            vec![]
        }

        async fn entries_by_action_prefix(&self, _prefix: &str, _limit: usize) -> Vec<AuditEntry> {
            vec![]
        }

        async fn entries_by_command_id(&self, command_id: &str, limit: usize) -> Vec<AuditEntry> {
            self.entries
                .lock()
                .unwrap()
                .iter()
                .filter(|e| e.command_id == command_id)
                .take(limit)
                .cloned()
                .collect()
        }

        async fn stats(&self) -> AuditStats {
            AuditStats::default()
        }

        async fn has_pending_batch(&self) -> bool {
            false
        }

        async fn log_event(&self, _action_type: &str, _session_id: &str, _details: &str) {}

        async fn log_start_if(
            &self,
            _level: AuditLevel,
            _command_id: &str,
            _session_id: &str,
            _action_type: &str,
        ) {
        }

        async fn log_complete_with_time(
            &self,
            _level: AuditLevel,
            _command_id: &str,
            _session_id: &str,
            _details: &str,
            _execution_time_ms: u64,
        ) {
        }

        async fn drain_batch(&self) -> Vec<AuditEntry> {
            vec![]
        }

        async fn drain_all(&self) -> Vec<AuditEntry> {
            vec![]
        }

        async fn record_session_event(&self, _entry: SessionAuditEntry) {}
    }

    /// 기본 AppState를 생성하는 헬퍼 (automation.audit_logger = None)
    fn fixture_state_no_logger() -> AppState {
        let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("in-memory sqlite"));
        let (event_tx, _) = broadcast::channel(16);
        AppState::with_core(storage, event_tx)
    }

    /// audit_logger가 설정된 AppState를 생성하는 헬퍼
    fn fixture_state_with_logger(logger: Arc<dyn AuditLogPort>) -> AppState {
        let mut state = fixture_state_no_logger();
        state.automation.audit_logger = Some(logger);
        state
    }

    /// 테스트 1: 필터 없이 모든 항목을 반환합니다.
    ///
    /// 5개의 항목이 있을 때, command_id 쿼리 없이 요청하면 5개가 반환됩니다.
    #[tokio::test]
    async fn audit_export_returns_all_entries_when_no_filter() {
        let entries: Vec<AuditEntry> = (0..5)
            .map(|i| make_entry("cmd-any", &format!("action-{i}")))
            .collect();
        let logger = SeedableAuditLog::with_entries(entries);
        let state = fixture_state_with_logger(logger);

        let query = AuditExportQuery {
            command_id: None,
            status: None,
            limit: None,
        };
        let resp = export_audit(State(state), Query(query)).await.unwrap().0;
        assert_eq!(resp.len(), 5);
    }

    /// 테스트 2: command_id로 필터링합니다.
    ///
    /// command_id "cmd-X"인 3개의 항목과 다른 command_id인 2개가 있을 때,
    /// ?command_id=cmd-X 쿼리로 3개만 반환됩니다.
    #[tokio::test]
    async fn audit_export_filters_by_command_id() {
        let mut entries: Vec<AuditEntry> = (0..3)
            .map(|i| make_entry("cmd-X", &format!("action-{i}")))
            .collect();
        entries.extend((0..2).map(|i| make_entry("cmd-Y", &format!("other-{i}"))));
        let logger = SeedableAuditLog::with_entries(entries);
        let state = fixture_state_with_logger(logger);

        let query = AuditExportQuery {
            command_id: Some("cmd-X".to_string()),
            status: None,
            limit: None,
        };
        let resp = export_audit(State(state), Query(query)).await.unwrap().0;
        assert_eq!(resp.len(), 3);
        assert!(resp.iter().all(|e| e.command_id == "cmd-X"));
    }

    /// 테스트 3: limit 파라미터를 준수합니다.
    ///
    /// 20개의 항목이 있을 때, ?limit=5 쿼리로 5개만 반환됩니다.
    #[tokio::test]
    async fn audit_export_respects_limit() {
        let entries: Vec<AuditEntry> = (0..20)
            .map(|i| make_entry("cmd-any", &format!("action-{i}")))
            .collect();
        let logger = SeedableAuditLog::with_entries(entries);
        let state = fixture_state_with_logger(logger);

        let query = AuditExportQuery {
            command_id: None,
            status: None,
            limit: Some(5),
        };
        let resp = export_audit(State(state), Query(query)).await.unwrap().0;
        assert_eq!(resp.len(), 5);
    }

    /// 테스트 4: limit이 1000을 초과하면 1000으로 제한합니다.
    ///
    /// ?limit=5000 쿼리로 요청해도 최대 1000개만 반환됩니다.
    #[tokio::test]
    async fn audit_export_caps_limit_at_1000() {
        // 1000개를 초과하는 항목을 시드합니다.
        let entries: Vec<AuditEntry> = (0..1200)
            .map(|i| make_entry("cmd-any", &format!("action-{i}")))
            .collect();
        let logger = SeedableAuditLog::with_entries(entries);
        let state = fixture_state_with_logger(logger);

        let query = AuditExportQuery {
            command_id: None,
            status: None,
            limit: Some(5000),
        };
        let resp = export_audit(State(state), Query(query)).await.unwrap().0;
        // SeedableAuditLog.recent_entries는 clamped limit을 준수합니다.
        assert_eq!(resp.len(), 1000);
    }

    /// 테스트 5: audit_logger가 None일 때 503을 반환합니다.
    #[tokio::test]
    async fn audit_export_returns_503_when_logger_none() {
        let state = fixture_state_no_logger();
        let query = AuditExportQuery {
            command_id: None,
            status: None,
            limit: None,
        };
        let err = export_audit(State(state), Query(query)).await.unwrap_err();
        assert!(
            matches!(err, ApiError::ServiceUnavailable(_)),
            "expected ServiceUnavailable, got {err:?}"
        );
    }

    /// 테스트 6: 빈 command_id는 필터로 처리되지 않습니다 (recent_entries 호출).
    #[tokio::test]
    async fn audit_export_empty_command_id_falls_back_to_recent() {
        let entries: Vec<AuditEntry> = vec![
            make_entry("cmd-A", "action-0"),
            make_entry("cmd-B", "action-1"),
        ];
        let logger = SeedableAuditLog::with_entries(entries);
        let state = fixture_state_with_logger(logger);

        let query = AuditExportQuery {
            command_id: Some("".to_string()), // 빈 문자열 → recent_entries 사용
            status: None,
            limit: None,
        };
        let resp = export_audit(State(state), Query(query)).await.unwrap().0;
        // 빈 command_id는 필터로 처리되지 않으므로 전체 2개를 반환합니다.
        assert_eq!(resp.len(), 2);
    }

    /// 테스트 7: limit 기본값은 100입니다.
    #[tokio::test]
    async fn audit_export_default_limit_is_100() {
        let entries: Vec<AuditEntry> = (0..200)
            .map(|i| make_entry("cmd-any", &format!("a-{i}")))
            .collect();
        let logger = SeedableAuditLog::with_entries(entries);
        let state = fixture_state_with_logger(logger);

        let query = AuditExportQuery {
            command_id: None,
            status: None,
            limit: None, // 기본값 사용
        };
        let resp = export_audit(State(state), Query(query)).await.unwrap().0;
        assert_eq!(resp.len(), 100);
    }

    /// 테스트 8: logger가 있고 항목이 없으면 빈 배열을 반환합니다.
    #[tokio::test]
    async fn audit_export_empty_log_returns_empty_vec() {
        let logger = SeedableAuditLog::empty();
        let state = fixture_state_with_logger(logger);

        let query = AuditExportQuery {
            command_id: None,
            status: None,
            limit: None,
        };
        let resp = export_audit(State(state), Query(query)).await.unwrap().0;
        assert!(resp.is_empty());
    }
}
