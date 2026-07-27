//! `GET /api/audit/export` — audit entry export with optional filtering.
//!
//! Spec §5.9 / D25 / NV1. Supports `command_id` + `status` (reserved) +
//! `limit` query params. Response: `Vec<AuditExportEntryDto>` newest-first.
//! DoS cap: limit clamped to 1000.
//!
//! 503 when `automation.audit_logger` is None (audit logger not configured).

use axum::extract::{Query, State};
use axum::Json;

use maekon_api_contracts::audit_export::{
    AuditExportEntryDto, AuditExportQuery, ExternalChatEgressOracleDto,
};
use maekon_core::models::audit::AuditEntry;

use crate::error::ApiError;
use crate::AppState;

fn detail_value<'a>(details: &'a str, key: &str) -> Option<&'a str> {
    details.split_whitespace().find_map(|part| {
        let (candidate, value) = part.split_once('=')?;
        (candidate == key).then_some(value)
    })
}

fn detail_bool(details: &str, key: &str) -> Option<bool> {
    match detail_value(details, key)? {
        "1" => Some(true),
        "0" => Some(false),
        _ => None,
    }
}

fn external_chat_egress_oracle(entry: &AuditEntry) -> Option<ExternalChatEgressOracleDto> {
    if entry.action_type != "privacy.external_llm.allowed" {
        return None;
    }
    let details = entry.details.as_deref()?;
    if detail_value(details, "o")? != "1" {
        return None;
    }

    Some(ExternalChatEgressOracleDto {
        version: 1,
        attachment_count: detail_value(details, "n")?.parse().ok()?,
        context_present: detail_bool(details, "cp")?,
        context_changed: detail_bool(details, "cc")?,
        attachments_changed: detail_bool(details, "ac")?,
        attachments_with_inline_data_before: detail_value(details, "ib")?.parse().ok()?,
        attachments_with_inline_data_after: detail_value(details, "ia")?.parse().ok()?,
        envelope_changed: detail_bool(details, "ec")?,
    })
}

/// `GET /api/audit/export` handler
///
/// When the `command_id` query parameter is present, returns entries filtered by
/// that command_id. Otherwise returns the most recent entries. The `limit`
/// parameter is capped at 1000.
///
/// #8114: the previously-reserved `status` parameter now routes through the
/// port's `entries_by_status` query (`Completed`/`Failed`/`Denied`/`Timeout`/
/// `Started`), giving the durable audit list the exact status-filter
/// semantics the in-memory automation buffer list has (full-window query, not
/// a post-fetch filter). Invalid tokens are a 400, matching the buffer list.
/// `command_id` takes precedence when both are present.
///
/// # Errors
/// - `400 Bad Request`: unknown `status` token
/// - `503 Service Unavailable`: when `automation.audit_logger` is None
pub async fn export_audit(
    State(state): State<AppState>,
    Query(query): Query<AuditExportQuery>,
) -> Result<Json<Vec<AuditExportEntryDto>>, ApiError> {
    let limit = query.limit.unwrap_or(100).min(1000);
    let Some(audit_logger) = state.automation.audit_logger.as_ref() else {
        return Err(ApiError::ServiceUnavailable(
            "audit logger not configured".into(),
        ));
    };
    let status_filter = query.status.as_deref().filter(|s| !s.is_empty());
    let entries = match (&query.command_id, status_filter) {
        (Some(cmd_id), _) if !cmd_id.is_empty() => {
            audit_logger.entries_by_command_id(cmd_id, limit).await
        }
        (_, Some(status_token)) => {
            let status =
                crate::services::automation_service::helpers::parse_audit_status(status_token)?;
            audit_logger.entries_by_status(&status, limit).await
        }
        _ => audit_logger.recent_entries(limit).await,
    };
    Ok(Json(
        entries
            .into_iter()
            .map(|entry| {
                let privacy_oracle = external_chat_egress_oracle(&entry);
                AuditExportEntryDto {
                    entry_id: entry.entry_id,
                    timestamp: entry.timestamp.to_rfc3339(),
                    command_id: entry.command_id,
                    action_type: entry.action_type,
                    status: format!("{:?}", entry.status),
                    execution_time_ms: entry.execution_time_ms,
                    privacy_oracle,
                }
            })
            .collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use chrono::Utc;
    use maekon_core::id_generation::generate_id;
    use maekon_core::models::ai_session::SessionAuditEntry;
    use maekon_core::models::audit::{AuditEntry, AuditLevel, AuditStats, AuditStatus};
    use maekon_core::ports::audit_log::AuditLogPort;
    use std::sync::{Arc, Mutex};

    /// Test-only `AuditLogPort` implementation — supports `recent_entries` and
    /// `entries_by_command_id` queries over seeded entries.
    struct SeedableAuditLog {
        entries: Mutex<Vec<AuditEntry>>,
    }

    impl SeedableAuditLog {
        /// Creates a new instance initialized with the given entries.
        fn with_entries(entries: Vec<AuditEntry>) -> Arc<Self> {
            Arc::new(Self {
                entries: Mutex::new(entries),
            })
        }

        fn empty() -> Arc<Self> {
            Self::with_entries(vec![])
        }
    }

    /// Helper that builds an `AuditEntry` with the given command_id and action_type.
    fn make_entry(command_id: &str, action_type: &str) -> AuditEntry {
        AuditEntry {
            entry_id: generate_id("aud"),
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

        async fn entries_by_status(&self, status: &AuditStatus, limit: usize) -> Vec<AuditEntry> {
            // Mirror the real port contract (a fake returning `vec![]` would
            // make the #8114 status-filter test pass vacuously).
            self.entries
                .lock()
                .unwrap()
                .iter()
                .filter(|e| e.status == *status)
                .take(limit)
                .cloned()
                .collect()
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

    /// Helper that builds a default `AppState` (automation.audit_logger = None).
    fn fixture_state_no_logger() -> AppState {
        crate::test_local_auth::test_app_state()
    }

    /// Helper that builds an `AppState` with the audit_logger configured.
    fn fixture_state_with_logger(logger: Arc<dyn AuditLogPort>) -> AppState {
        let mut state = fixture_state_no_logger();
        state.automation.audit_logger = Some(logger);
        state
    }

    /// Test 1: returns all entries when no filter is applied.
    ///
    /// Given 5 entries, a request without a command_id query returns all 5.
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

    /// #8114: the durable list's status filter — previously a documented
    /// no-op — now routes through the port's `entries_by_status` query,
    /// giving the Audit Log UI the same status-filter semantics the
    /// in-memory buffer list has.
    #[tokio::test]
    async fn audit_export_filters_by_status_within_window() {
        let mut entries: Vec<AuditEntry> = (0..3)
            .map(|i| make_entry("cmd-s", &format!("done-{i}")))
            .collect();
        let mut failed = make_entry("cmd-s", "boom");
        failed.status = AuditStatus::Failed;
        entries.push(failed);
        let logger = SeedableAuditLog::with_entries(entries);
        let state = fixture_state_with_logger(logger);

        let query = AuditExportQuery {
            command_id: None,
            status: Some("Failed".to_string()),
            limit: None,
        };
        let resp = export_audit(State(state.clone()), Query(query))
            .await
            .unwrap()
            .0;
        assert_eq!(resp.len(), 1);
        assert_eq!(resp[0].status, "Failed");
        assert_eq!(resp[0].action_type, "boom");

        // Empty string is treated as absent (parity with command_id).
        let query = AuditExportQuery {
            command_id: None,
            status: Some(String::new()),
            limit: None,
        };
        let resp = export_audit(State(state), Query(query)).await.unwrap().0;
        assert_eq!(resp.len(), 4);
    }

    /// Test 2: filters by command_id.
    ///
    /// Given 3 entries with command_id "cmd-X" and 2 with a different command_id,
    /// a `?command_id=cmd-X` query returns only the 3 matching entries.
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

    /// Test 3: respects the limit parameter.
    ///
    /// Given 20 entries, a `?limit=5` query returns only 5.
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

    /// Test 4: caps the limit at 1000 when it exceeds 1000.
    ///
    /// Even with a `?limit=5000` query, at most 1000 entries are returned.
    #[tokio::test]
    async fn audit_export_caps_limit_at_1000() {
        // Seed more than 1000 entries.
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
        // SeedableAuditLog.recent_entries honors the clamped limit.
        assert_eq!(resp.len(), 1000);
    }

    /// Test 5: returns 503 when audit_logger is None.
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

    /// Test 6: an empty command_id is not treated as a filter (calls recent_entries).
    #[tokio::test]
    async fn audit_export_empty_command_id_falls_back_to_recent() {
        let entries: Vec<AuditEntry> = vec![
            make_entry("cmd-A", "action-0"),
            make_entry("cmd-B", "action-1"),
        ];
        let logger = SeedableAuditLog::with_entries(entries);
        let state = fixture_state_with_logger(logger);

        let query = AuditExportQuery {
            command_id: Some("".to_string()), // empty string → uses recent_entries
            status: None,
            limit: None,
        };
        let resp = export_audit(State(state), Query(query)).await.unwrap().0;
        // An empty command_id is not treated as a filter, so all 2 entries are returned.
        assert_eq!(resp.len(), 2);
    }

    /// Test 7: the default limit is 100.
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
            limit: None, // use the default
        };
        let resp = export_audit(State(state), Query(query)).await.unwrap().0;
        assert_eq!(resp.len(), 100);
    }

    /// Test 8: returns an empty array when a logger is present but has no entries.
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

    /// Test 9: the wire response structurally excludes sensitive runtime fields.
    #[tokio::test]
    async fn audit_export_omits_session_and_free_form_details() {
        let mut entry = make_entry("cmd-safe", "MouseClick");
        entry.session_id = "session-sensitive".to_string();
        entry.details = Some("consent_id=raw-sensitive-marker".to_string());
        let state = fixture_state_with_logger(SeedableAuditLog::with_entries(vec![entry]));

        let query = AuditExportQuery {
            command_id: None,
            status: None,
            limit: None,
        };
        let resp = export_audit(State(state), Query(query)).await.unwrap().0;
        let json = serde_json::to_value(&resp).unwrap();
        let row = json[0].as_object().unwrap();

        assert_eq!(
            row.keys()
                .map(String::as_str)
                .collect::<std::collections::BTreeSet<_>>(),
            std::collections::BTreeSet::from([
                "action_type",
                "command_id",
                "entry_id",
                "execution_time_ms",
                "status",
                "timestamp",
            ])
        );
        let serialized = serde_json::to_string(&json).unwrap();
        assert!(!serialized.contains("session-sensitive"));
        assert!(!serialized.contains("raw-sensitive-marker"));
        assert!(!serialized.contains("session_id"));
        assert!(!serialized.contains("details"));
    }

    #[tokio::test]
    async fn audit_export_exposes_only_structured_external_chat_oracle() {
        let mut entry = make_entry("session-safe", "privacy.external_llm.allowed");
        entry.details = Some(
            "o=1 p=conversation u=chat a=Code z=42 n=1 cp=1 cc=1 ac=1 \
             ib=1 ia=0 ec=1 secret=raw-sensitive-marker"
                .to_string(),
        );
        let state = fixture_state_with_logger(SeedableAuditLog::with_entries(vec![entry]));

        let query = AuditExportQuery {
            command_id: Some("session-safe".to_string()),
            status: None,
            limit: None,
        };
        let resp = export_audit(State(state), Query(query)).await.unwrap().0;
        let oracle = resp[0]
            .privacy_oracle
            .as_ref()
            .expect("allowed external chat should expose a structured safe oracle");

        assert_eq!(oracle.version, 1);
        assert_eq!(oracle.attachment_count, 1);
        assert!(oracle.context_present);
        assert!(oracle.context_changed);
        assert!(oracle.attachments_changed);
        assert_eq!(oracle.attachments_with_inline_data_before, 1);
        assert_eq!(oracle.attachments_with_inline_data_after, 0);
        assert!(oracle.envelope_changed);

        let serialized = serde_json::to_string(&resp).unwrap();
        assert!(!serialized.contains("raw-sensitive-marker"));
        assert!(!serialized.contains("\"details\""));
        assert!(!serialized.contains("\"app\""));
        assert!(!serialized.contains("\"content_chars\""));
    }
}
