//! Bridge from external gRPC requests into the existing AuditLogger.
//! Uses AuditEntry.details as a JSON blob (no schema change).

use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;

use oneshim_core::models::audit::{AuditLevel, AuditStatus};
use oneshim_core::ports::audit_log::AuditLogPort;

use super::conn_info::{AuthContext, AuthType};

/// External gRPC audit detail (serialized into AuditEntry.details as JSON).
#[derive(Debug, Serialize)]
pub(crate) struct ExternalGrpcAuditDetails<'a> {
    pub(crate) transport: &'static str, // always "external"
    pub(crate) remote_addr: String,
    pub(crate) auth_type: &'static str,
    pub(crate) operation: &'a str,
    pub(crate) result: &'static str,
    pub(crate) request_size_bytes: Option<u64>,
    pub(crate) response_size_bytes: Option<u64>,
    pub(crate) failure_reason: Option<&'a str>,
    pub(crate) jti: Option<&'a str>,
    /// Count of stream messages yielded by the handler (streaming RPCs only).
    /// `None` for unary RPCs + Started/Failed (AuthLayer) paths. Populated by
    /// `CountingStream` via request extensions (spec §2.4).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) response_message_count: Option<u64>,
    /// Raw tonic::Code as u32. Populated by AuditBridge completion paths so
    /// security dashboards can disambiguate Unauthenticated (16) vs
    /// PermissionDenied (7) — both otherwise collapse into AuditStatus::Denied.
    /// None for Success paths (status already conveys success). Task 0.6 wires
    /// the producer; this field is None at all construction sites until then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) grpc_status_code: Option<u32>,
}

pub struct AuditBridge {
    port: Arc<dyn AuditLogPort>,
}

impl AuditBridge {
    pub fn new(port: Arc<dyn AuditLogPort>) -> Self {
        Self { port }
    }

    /// Record one external gRPC request. Returns the command_id (for response header).
    ///
    /// Uses `log_complete_with_time` so that `command_id`, `session_id`,
    /// `details`, and `execution_time_ms` are all preserved in the stored entry.
    /// The `status` and `failure_reason` are encoded inside the JSON details blob.
    ///
    /// # Parameters (new in Task 0.6, per spec §5.5 + U5)
    /// - `command_id`: when `Some`, overrides `ctx.command_id` in the stored entry
    ///   and the return value. Pass `None` to fall back to `ctx.command_id`.
    #[allow(clippy::too_many_arguments)]
    pub async fn record(
        &self,
        ctx: &AuthContext,
        remote_addr: String,
        operation: &str,
        result: &'static str,
        status: AuditStatus,
        duration: Duration,
        request_size: Option<u64>,
        response_size: Option<u64>,
        failure_reason: Option<&str>,
        command_id: Option<String>, // NEW per spec §5.5 + U5
    ) -> String {
        let effective_cmd_id: &str = command_id.as_deref().unwrap_or(&ctx.command_id);
        let details = ExternalGrpcAuditDetails {
            transport: "external",
            remote_addr,
            auth_type: match ctx.auth_type {
                AuthType::Jwt => "jwt",
                AuthType::Mtls => "mtls",
                AuthType::JwtAndMtls => "jwt+mtls",
            },
            operation,
            result,
            request_size_bytes: request_size,
            response_size_bytes: response_size,
            failure_reason,
            jti: ctx.jti.as_deref(),
            response_message_count: None,
            grpc_status_code: None, // unused in Started/Failed paths (AuditLayer record_completion populates)
        };
        let details_json =
            serde_json::to_string(&details).unwrap_or_else(|e| format!("{{\"err\":\"{e}\"}}"));
        // Encode the status label into the action_type prefix so that consumers
        // can distinguish completed from failed entries without parsing JSON.
        let action_type = match status {
            AuditStatus::Completed => "external_grpc_completed",
            AuditStatus::Failed => "external_grpc_failed",
            AuditStatus::Denied => "external_grpc_denied",
            AuditStatus::Started => "external_grpc_started",
            AuditStatus::Timeout => "external_grpc_timeout",
        };
        self.port
            .log_complete_with_time(
                AuditLevel::Full,
                effective_cmd_id,
                &ctx.client_id,
                &details_json,
                duration.as_millis() as u64,
            )
            .await;
        // Also emit a plain log_event so that the AuditLogger's action_type-prefix
        // query surface returns results for callers using `entries_by_action_prefix`.
        self.port
            .log_event(action_type, &ctx.client_id, &details_json)
            .await;
        command_id.unwrap_or_else(|| ctx.command_id.clone())
    }

    /// Record a completion audit entry. Complements `record(Started/Failed)`.
    /// Status mapping per Task 13 spec §2.2:
    /// - Ok → `AuditStatus::Completed`
    /// - PermissionDenied → `AuditStatus::Denied`
    /// - Cancelled/DeadlineExceeded → `AuditStatus::Timeout`
    /// - Other error → `AuditStatus::Failed` (+ failure_reason = status message)
    /// - Panic → `AuditStatus::Failed` (+ failure_reason = "handler_panic")
    ///
    /// # Parameters (new in Task 0.6, per spec §5.5 + U5 + D26)
    /// - `command_id`: when `Some`, overrides `ctx.command_id` in the stored entry
    ///   and the return value. Pass `None` to fall back to `ctx.command_id`.
    /// - `grpc_status_code`: raw `tonic::Code` as `u32`. Stored in the JSON details
    ///   blob so dashboards can disambiguate Unauthenticated (16) vs PermissionDenied (7).
    ///   Pass `None` for success paths (status already conveys success).
    #[allow(clippy::too_many_arguments)]
    pub async fn record_completion(
        &self,
        ctx: &AuthContext,
        remote_addr: String,
        operation: &str,
        status: AuditStatus,
        duration: Duration,
        response_message_count: Option<u64>,
        failure_reason: Option<&str>,
        command_id: Option<String>,    // NEW per spec §5.5 + U5
        grpc_status_code: Option<u32>, // NEW per spec §5.5 + D26
    ) -> String {
        let effective_cmd_id: &str = command_id.as_deref().unwrap_or(&ctx.command_id);
        let result = match status {
            AuditStatus::Completed => "ok",
            AuditStatus::Denied => "denied",
            AuditStatus::Timeout => "timeout",
            AuditStatus::Failed => "error",
            AuditStatus::Started => "ok", // not expected here but kept exhaustive
        };
        let details = ExternalGrpcAuditDetails {
            transport: "external",
            remote_addr,
            auth_type: match ctx.auth_type {
                AuthType::Jwt => "jwt",
                AuthType::Mtls => "mtls",
                AuthType::JwtAndMtls => "jwt+mtls",
            },
            operation,
            result,
            request_size_bytes: None,
            response_size_bytes: None,
            failure_reason,
            jti: ctx.jti.as_deref(),
            response_message_count,
            grpc_status_code,
        };
        let details_json =
            serde_json::to_string(&details).unwrap_or_else(|e| format!("{{\"err\":\"{e}\"}}"));
        let action_type = match status {
            AuditStatus::Completed => "external_grpc_completed",
            AuditStatus::Failed => "external_grpc_failed",
            AuditStatus::Denied => "external_grpc_denied",
            AuditStatus::Timeout => "external_grpc_timeout",
            AuditStatus::Started => "external_grpc_started",
        };
        self.port
            .log_complete_with_time(
                AuditLevel::Full,
                effective_cmd_id,
                &ctx.client_id,
                &details_json,
                duration.as_millis() as u64,
            )
            .await;
        self.port
            .log_event(action_type, &ctx.client_id, &details_json)
            .await;
        command_id.unwrap_or_else(|| ctx.command_id.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::Mutex;

    use oneshim_core::models::ai_session::SessionAuditEntry;
    use oneshim_core::models::audit::{AuditStats, AuditStatus};

    use chrono::Utc;
    use ulid::Ulid;

    use oneshim_core::models::audit::AuditEntry;

    /// Lightweight mock that captures `log_complete_with_time` calls as `AuditEntry`
    /// values so that tests can assert on `command_id`, `details`, and derived `status`.
    struct MockAuditLog {
        entries: Mutex<Vec<AuditEntry>>,
    }

    impl MockAuditLog {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                entries: Mutex::new(vec![]),
            })
        }
    }

    #[async_trait]
    impl AuditLogPort for MockAuditLog {
        // ── Query stubs ──
        async fn pending_count(&self) -> usize {
            0
        }
        async fn recent_entries(&self, _limit: usize) -> Vec<AuditEntry> {
            vec![]
        }
        async fn entries_by_status(&self, _status: &AuditStatus, _limit: usize) -> Vec<AuditEntry> {
            vec![]
        }
        async fn entries_by_action_prefix(&self, _prefix: &str, _limit: usize) -> Vec<AuditEntry> {
            vec![]
        }
        async fn entries_by_command_id(&self, _cmd_id: &str, _limit: usize) -> Vec<AuditEntry> {
            vec![]
        }
        async fn stats(&self) -> AuditStats {
            AuditStats::default()
        }
        async fn has_pending_batch(&self) -> bool {
            false
        }

        // ── Mutation: log_event is a no-op in the mock (we capture via log_complete_with_time) ──
        async fn log_event(&self, _action_type: &str, _session_id: &str, _details: &str) {}

        async fn log_start_if(
            &self,
            _level: AuditLevel,
            _command_id: &str,
            _session_id: &str,
            _action_type: &str,
        ) {
        }

        /// Primary capture point: stores a full `AuditEntry` from the supplied args.
        /// The `status` is inferred from the JSON `result` field inside `details`
        /// so that tests can assert on `entries[0].status`.
        async fn log_complete_with_time(
            &self,
            _level: AuditLevel,
            command_id: &str,
            session_id: &str,
            details: &str,
            execution_time_ms: u64,
        ) {
            // Derive status from the serialised details JSON so callers that pass
            // AuditStatus::Failed through the result field can observe it here.
            let status = serde_json::from_str::<serde_json::Value>(details)
                .ok()
                .and_then(|v| {
                    v.get("result").and_then(|r| r.as_str()).map(|r| match r {
                        "ok" => AuditStatus::Completed,
                        _ => AuditStatus::Failed,
                    })
                })
                .unwrap_or(AuditStatus::Completed);

            self.entries.lock().unwrap().push(AuditEntry {
                entry_id: Ulid::new().to_string(),
                timestamp: Utc::now(),
                session_id: session_id.to_string(),
                command_id: command_id.to_string(),
                action_type: "external_grpc".to_string(),
                status,
                details: Some(details.to_string()),
                execution_time_ms: Some(execution_time_ms),
            });
        }

        // ── Drain stubs ──
        async fn drain_batch(&self) -> Vec<AuditEntry> {
            vec![]
        }
        async fn drain_all(&self) -> Vec<AuditEntry> {
            vec![]
        }

        // Default no-op for record_session_event is inherited.
        async fn record_session_event(&self, _entry: SessionAuditEntry) {}
    }

    fn mk_ctx() -> AuthContext {
        AuthContext {
            auth_type: AuthType::Jwt,
            client_id: "user-1".into(),
            jti: Some("jti-abc".into()),
            command_id: Ulid::new().to_string(),
        }
    }

    #[tokio::test]
    async fn records_completed_entry_with_json_details() {
        let mock = MockAuditLog::new();
        let bridge = AuditBridge::new(mock.clone());
        let ctx = mk_ctx();
        let cid = bridge
            .record(
                &ctx,
                "127.0.0.1:1234".into(),
                "/DashboardService/SubscribeEvents",
                "ok",
                AuditStatus::Completed,
                Duration::from_millis(42),
                Some(100),
                None,
                None,
                None,
            )
            .await;
        assert_eq!(cid, ctx.command_id);
        let entries = mock.entries.lock().unwrap();
        assert_eq!(entries.len(), 1);
        let detail: serde_json::Value =
            serde_json::from_str(entries[0].details.as_ref().unwrap()).unwrap();
        assert_eq!(detail["transport"], "external");
        assert_eq!(detail["auth_type"], "jwt");
        assert_eq!(detail["operation"], "/DashboardService/SubscribeEvents");
        assert_eq!(detail["result"], "ok");
    }

    #[tokio::test]
    async fn records_failure_entry_with_reason() {
        let mock = MockAuditLog::new();
        let bridge = AuditBridge::new(mock.clone());
        let ctx = mk_ctx();
        bridge
            .record(
                &ctx,
                "127.0.0.1:5000".into(),
                "/DashboardService/SubscribeMetrics",
                "auth_failed",
                AuditStatus::Failed,
                Duration::from_millis(10),
                Some(0),
                None,
                Some("invalid_jwt"),
                None,
            )
            .await;
        let entries = mock.entries.lock().unwrap();
        let detail: serde_json::Value =
            serde_json::from_str(entries[0].details.as_ref().unwrap()).unwrap();
        assert_eq!(detail["failure_reason"], "invalid_jwt");
        // "auth_failed" != "ok" → MockAuditLog infers AuditStatus::Failed.
        assert!(matches!(entries[0].status, AuditStatus::Failed));
    }

    #[tokio::test]
    async fn command_id_round_trips_through_entry() {
        let mock = MockAuditLog::new();
        let bridge = AuditBridge::new(mock.clone());
        let ctx = mk_ctx();
        let cid = bridge
            .record(
                &ctx,
                "10.0.0.1:8080".into(),
                "/op",
                "ok",
                AuditStatus::Completed,
                Duration::from_millis(1),
                None,
                None,
                None,
                None,
            )
            .await;
        let entries = mock.entries.lock().unwrap();
        assert_eq!(entries[0].command_id, cid);
    }

    // ── record_completion status-mapping tests (Task 13 §2.2) ────────────

    #[tokio::test]
    async fn record_completion_maps_completed_to_ok() {
        let mock = MockAuditLog::new();
        let bridge = AuditBridge::new(mock.clone());
        let ctx = mk_ctx();
        bridge
            .record_completion(
                &ctx,
                "127.0.0.1:5000".into(),
                "/svc/op",
                AuditStatus::Completed,
                Duration::from_millis(12),
                Some(5),
                None,
                None,
                None,
            )
            .await;
        let entries = mock.entries.lock().unwrap();
        assert_eq!(entries.len(), 1);
        let d: serde_json::Value =
            serde_json::from_str(entries[0].details.as_ref().unwrap()).unwrap();
        assert_eq!(d["result"], "ok");
        assert_eq!(d["response_message_count"], 5);
    }

    #[tokio::test]
    async fn record_completion_maps_denied() {
        let mock = MockAuditLog::new();
        let bridge = AuditBridge::new(mock.clone());
        let ctx = mk_ctx();
        bridge
            .record_completion(
                &ctx,
                "127.0.0.1:5000".into(),
                "/svc/op",
                AuditStatus::Denied,
                Duration::from_millis(3),
                None,
                Some("not authorized"),
                None,
                None,
            )
            .await;
        let entries = mock.entries.lock().unwrap();
        let d: serde_json::Value =
            serde_json::from_str(entries[0].details.as_ref().unwrap()).unwrap();
        assert_eq!(d["result"], "denied");
        assert_eq!(d["failure_reason"], "not authorized");
    }

    #[tokio::test]
    async fn record_completion_maps_timeout() {
        let mock = MockAuditLog::new();
        let bridge = AuditBridge::new(mock.clone());
        let ctx = mk_ctx();
        bridge
            .record_completion(
                &ctx,
                "127.0.0.1:5000".into(),
                "/svc/op",
                AuditStatus::Timeout,
                Duration::from_millis(60_000),
                None,
                Some("deadline_exceeded"),
                None,
                None,
            )
            .await;
        let entries = mock.entries.lock().unwrap();
        let d: serde_json::Value =
            serde_json::from_str(entries[0].details.as_ref().unwrap()).unwrap();
        assert_eq!(d["result"], "timeout");
    }

    #[tokio::test]
    async fn record_completion_maps_failed() {
        let mock = MockAuditLog::new();
        let bridge = AuditBridge::new(mock.clone());
        let ctx = mk_ctx();
        bridge
            .record_completion(
                &ctx,
                "127.0.0.1:5000".into(),
                "/svc/op",
                AuditStatus::Failed,
                Duration::from_millis(15),
                None,
                Some("internal"),
                None,
                None,
            )
            .await;
        let entries = mock.entries.lock().unwrap();
        let d: serde_json::Value =
            serde_json::from_str(entries[0].details.as_ref().unwrap()).unwrap();
        assert_eq!(d["result"], "error");
    }

    #[tokio::test]
    async fn record_completion_count_absent_when_none() {
        let mock = MockAuditLog::new();
        let bridge = AuditBridge::new(mock.clone());
        let ctx = mk_ctx();
        bridge
            .record_completion(
                &ctx,
                "127.0.0.1:5000".into(),
                "/svc/op",
                AuditStatus::Completed,
                Duration::from_millis(1),
                None,
                None,
                None,
                None,
            )
            .await;
        let entries = mock.entries.lock().unwrap();
        let d: serde_json::Value =
            serde_json::from_str(entries[0].details.as_ref().unwrap()).unwrap();
        // skip_serializing_if None → absent key
        assert!(d.get("response_message_count").is_none());
    }

    // ── Task 0.6: command_id override + grpc_status_code propagation ────────

    /// Verifies that `record_completion` accepts `command_id` (Option<String>) and
    /// `grpc_status_code` (Option<u32>) and propagates them correctly.
    ///
    /// Spec §5.5 + D26 + U5: command_id arg overrides ctx.command_id; grpc_status_code
    /// flows into ExternalGrpcAuditDetails and is serialized into the JSON details blob.
    #[tokio::test]
    async fn record_completion_accepts_command_id_and_grpc_status_code() {
        let mock = MockAuditLog::new();
        let bridge = AuditBridge::new(mock.clone());
        let ctx = mk_ctx();
        let explicit_cmd_id = "req-abc-123".to_string();
        bridge
            .record_completion(
                &ctx,
                "127.0.0.1:1234".to_string(),
                "/Service/Method",
                AuditStatus::Denied,
                Duration::from_millis(42),
                Some(5u64),                    // response_message_count
                None,                          // failure_reason
                Some(explicit_cmd_id.clone()), // command_id (NEW §5.5 + U5)
                Some(7u32), // grpc_status_code (NEW §5.5 + D26) — PermissionDenied
            )
            .await;

        let entries = mock.entries.lock().unwrap();
        let entry = entries.last().expect("one record captured");
        // grpc_status_code must be serialized into the details JSON blob.
        let details: serde_json::Value =
            serde_json::from_str(entry.details.as_deref().unwrap_or("{}"))
                .expect("parse details JSON");
        assert_eq!(
            details["grpc_status_code"], 7,
            "grpc_status_code must propagate into details JSON"
        );
        // command_id arg must override ctx.command_id in the stored entry.
        assert_eq!(
            entry.command_id, explicit_cmd_id,
            "command_id arg must override ctx.command_id"
        );
    }

    /// Verifies that `record` accepts the new `command_id` (Option<String>) trailing arg.
    ///
    /// Spec §5.5 + U5: when Some, overrides ctx.command_id in the stored entry and
    /// return value; when None, falls back to ctx.command_id.
    #[tokio::test]
    async fn record_accepts_command_id_override() {
        let mock = MockAuditLog::new();
        let bridge = AuditBridge::new(mock.clone());
        let ctx = mk_ctx();
        let explicit_cmd_id = "override-cmd-456".to_string();
        let returned = bridge
            .record(
                &ctx,
                "127.0.0.1:9999".to_string(),
                "/Svc/Op",
                "ok",
                AuditStatus::Completed,
                Duration::from_millis(5),
                None,
                None,
                None,
                Some(explicit_cmd_id.clone()), // command_id (NEW §5.5 + U5)
            )
            .await;
        // Return value reflects the override.
        assert_eq!(returned, explicit_cmd_id);
        let entries = mock.entries.lock().unwrap();
        assert_eq!(entries.last().unwrap().command_id, explicit_cmd_id);
    }

    /// Verifies that passing `None` for command_id in `record` falls back to ctx.command_id.
    #[tokio::test]
    async fn record_command_id_none_falls_back_to_ctx() {
        let mock = MockAuditLog::new();
        let bridge = AuditBridge::new(mock.clone());
        let ctx = mk_ctx();
        let returned = bridge
            .record(
                &ctx,
                "127.0.0.1:9999".to_string(),
                "/Svc/Op",
                "ok",
                AuditStatus::Completed,
                Duration::from_millis(5),
                None,
                None,
                None,
                None, // command_id None → fallback to ctx.command_id
            )
            .await;
        assert_eq!(returned, ctx.command_id);
        let entries = mock.entries.lock().unwrap();
        assert_eq!(entries.last().unwrap().command_id, ctx.command_id);
    }

    // ── grpc_status_code field tests (Task 0.5 / spec §5.5 / D26 OQ15) ────

    // NOTE on deserialization tests:
    // Plan §5.5 / D26 template included tests for:
    //   - Deserializing older audit rows (without grpc_status_code field)
    //   - Tolerating future unknown fields
    // These cannot be exercised on ExternalGrpcAuditDetails directly because the
    // struct is Serialize-only (has lifetime-parameterized &str fields; deriving
    // Deserialize would require owning strings via String or Cow). In practice
    // nothing deserializes this struct — audit rows are stored as opaque JSON
    // in audit_log.details. Backward-compat concerns are therefore moot.
    //
    // If a future task introduces a read-path that deserializes audit details
    // (e.g. a `OwnedAuditDetails` type for DB reads), deserialization tests
    // should be added against THAT type.

    #[test]
    fn external_grpc_audit_details_serializes_grpc_status_code_when_some() {
        let d = ExternalGrpcAuditDetails {
            transport: "external",
            remote_addr: "127.0.0.1:1234".to_string(),
            auth_type: "jwt",
            operation: "/dashboard.v1.Foo/Bar",
            result: "denied",
            request_size_bytes: None,
            response_size_bytes: None,
            failure_reason: None,
            jti: None,
            response_message_count: None,
            grpc_status_code: Some(7), // PermissionDenied
        };
        let json = serde_json::to_value(&d).expect("serialize");
        assert_eq!(json["grpc_status_code"], 7);
    }

    #[test]
    fn external_grpc_audit_details_none_grpc_status_code_skipped_in_serialization() {
        let d = ExternalGrpcAuditDetails {
            transport: "external",
            remote_addr: "127.0.0.1:1234".to_string(),
            auth_type: "jwt",
            operation: "/dashboard.v1.Foo/Bar",
            result: "ok",
            request_size_bytes: None,
            response_size_bytes: None,
            failure_reason: None,
            jti: None,
            response_message_count: None,
            grpc_status_code: None,
        };
        let json = serde_json::to_string(&d).expect("serialize");
        assert!(
            !json.contains("grpc_status_code"),
            "None must skip; backward-compat for older audit rows: got {json}"
        );
    }
}
