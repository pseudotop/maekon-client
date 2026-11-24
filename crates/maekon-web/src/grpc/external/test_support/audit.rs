//! Audit fixtures and factories for external gRPC contract tests.

use std::sync::Arc;

use maekon_core::models::ai_session::SessionAuditEntry;
use maekon_core::models::audit::{AuditEntry, AuditLevel, AuditStats, AuditStatus};
use maekon_core::ports::audit_log::AuditLogPort;

// ── CapturingAudit — richer capture than MockRecorder ──────────────────────────
//
// `MockRecorder` captures durable `AuditEntry` rows for `AuditBridge` unit
// tests. `CapturingAudit` keeps a lighter projection for external gRPC
// integration tests, including raw gRPC status codes from JSON details.
// Both retain explicit typed statuses and support legacy completion calls.
// Their distinct capture shapes and call-site contracts are preserved (#7730).

/// A lightweight capture record used by Phase 9 integration tests to assert
/// on command_id, action_type, status, grpc_status_code, and execution timing.
///
/// `details` preserves the raw payload from legacy and typed audit calls so that
/// tests can inspect operation names (e.g. "SubscribeEvents") without re-parsing
/// the struct.
#[derive(Clone, Debug)]
pub struct CapturedEntry {
    pub command_id: String,
    pub action_type: String,
    pub status: AuditStatus,
    pub grpc_status_code: Option<u32>,
    pub execution_time_ms: u64,
    /// Raw `details` string passed by the audit bridge (JSON blob or empty).
    /// Populated by completion, typed-status and event calls; `None` for starts.
    pub details: Option<String>,
}

// Capture-only audit log retaining legacy and typed calls for integration tests.
//
// Structural rewrite (Task 9.0, CR4 / R2-NI1): replaces the previous
// action_type-as-command_id conflation with real command_id preservation and
// grpc_status_code JSON extraction from the details blob.  Unblocks Phase 9
// Tasks 9.1+ which assert command_id correlation and D26 raw-code visibility.
#[derive(Default)]
pub struct CapturingAudit {
    /// Shared capture buffer. Prefer `snapshot` when inspecting completed calls.
    pub entries: parking_lot::Mutex<Vec<CapturedEntry>>,
}

impl CapturingAudit {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            entries: parking_lot::Mutex::new(vec![]),
        })
    }

    /// Return a snapshot of all captured entries.  Used by Phase 9.1+ tests
    /// to assert command_id correlation and grpc_status_code visibility.
    pub fn snapshot(&self) -> Vec<CapturedEntry> {
        self.entries.lock().clone()
    }
}

#[async_trait::async_trait]
impl AuditLogPort for CapturingAudit {
    async fn log_event(&self, action_type: &str, _session_id: &str, details: &str) {
        // Preserve statuses for legacy event callers. AuditBridge uses the
        // explicit-status port method below for its durable rows.
        let status = match action_type {
            "external_grpc_started" => AuditStatus::Started,
            "external_grpc_failed" => AuditStatus::Failed,
            "external_grpc_denied" => AuditStatus::Denied,
            "external_grpc_timeout" => AuditStatus::Timeout,
            _ => AuditStatus::Completed,
        };
        self.entries.lock().push(CapturedEntry {
            command_id: String::new(),
            action_type: action_type.to_string(),
            status,
            grpc_status_code: None,
            execution_time_ms: 0,
            details: Some(details.to_string()),
        });
    }

    async fn log_start_if(
        &self,
        _level: AuditLevel,
        command_id: &str,
        _session_id: &str,
        action_type: &str,
    ) {
        self.entries.lock().push(CapturedEntry {
            command_id: command_id.to_string(),
            action_type: action_type.to_string(),
            status: AuditStatus::Started,
            grpc_status_code: None,
            execution_time_ms: 0,
            details: None,
        });
    }

    async fn log_complete_with_time(
        &self,
        _level: AuditLevel,
        command_id: &str,
        _session_id: &str,
        details: &str,
        execution_time_ms: u64,
    ) {
        let status = parse_status_from_details(details);
        let grpc_status_code = parse_grpc_status_code(details);
        self.entries.lock().push(CapturedEntry {
            command_id: command_id.to_string(),
            action_type: String::new(),
            status,
            grpc_status_code,
            execution_time_ms,
            details: Some(details.to_string()),
        });
    }

    // #6277: AuditBridge now emits ONE durable row via log_with_status_and_time
    // carrying the REAL status + action_type (replacing log_complete_with_time +
    // a separate log_event). Capture the true status/action_type here so the mock
    // reflects the new contract — the trait default would delegate to
    // log_complete_with_time and lose the Started/Denied/Failed/Timeout status.
    async fn log_with_status_and_time(
        &self,
        _level: AuditLevel,
        command_id: &str,
        _session_id: &str,
        action_type: &str,
        status: AuditStatus,
        details: &str,
        execution_time_ms: u64,
    ) {
        let grpc_status_code = parse_grpc_status_code(details);
        self.entries.lock().push(CapturedEntry {
            command_id: command_id.to_string(),
            action_type: action_type.to_string(),
            status,
            grpc_status_code,
            execution_time_ms,
            details: Some(details.to_string()),
        });
    }

    async fn pending_count(&self) -> usize {
        0
    }
    async fn recent_entries(&self, _limit: usize) -> Vec<AuditEntry> {
        vec![]
    }
    async fn entries_by_status(&self, _s: &AuditStatus, _l: usize) -> Vec<AuditEntry> {
        vec![]
    }
    async fn entries_by_action_prefix(&self, _p: &str, _l: usize) -> Vec<AuditEntry> {
        vec![]
    }
    async fn entries_by_command_id(&self, _cmd_id: &str, _limit: usize) -> Vec<AuditEntry> {
        vec![]
    }
    async fn stats(&self) -> AuditStats {
        Default::default()
    }
    async fn has_pending_batch(&self) -> bool {
        false
    }
    async fn drain_batch(&self) -> Vec<AuditEntry> {
        vec![]
    }
    async fn drain_all(&self) -> Vec<AuditEntry> {
        vec![]
    }
    async fn record_session_event(&self, _e: SessionAuditEntry) {}
}

/// Derive `AuditStatus` from the JSON `result` field in the details blob emitted
/// by `AuditBridge::record`.  Returns `Completed` for any unrecognized value.
fn parse_status_from_details(details: &str) -> AuditStatus {
    serde_json::from_str::<serde_json::Value>(details)
        .ok()
        .and_then(|v| v.get("result").and_then(|r| r.as_str().map(String::from)))
        .map(|s| match s.as_str() {
            "denied" => AuditStatus::Denied,
            "timeout" => AuditStatus::Timeout,
            "error" | "failed" | "auth_failed" => AuditStatus::Failed,
            _ => AuditStatus::Completed,
        })
        .unwrap_or(AuditStatus::Completed)
}

fn parse_grpc_status_code(details: &str) -> Option<u32> {
    let value: serde_json::Value = serde_json::from_str(details).ok()?;
    u32::try_from(value.get("grpc_status_code")?.as_u64()?).ok()
}

// ── No-op audit and durable capture fixtures ─────────────────────────────────
//
// #7730: shared test-binary helpers, de-duplicated from verbatim copies that
// previously lived in both `tests/external_grpc_integration.rs` and
// `tests/external_grpc_stress.rs`. NOTE: `in_memory_storage` (the third
// duplicated helper named in #7730) deliberately does NOT live here — it
// needs `maekon-storage`, an adapter crate, and `scripts/check-crate-
// boundaries.sh` forbids `maekon-web` from taking `maekon-storage` as a
// normal (even optional/feature-gated) dependency — only `dev-dependencies`
// are exempt (see that script's comment). It is instead shared via
// `tests/support/in_memory_storage.rs`, `#[path]`-included independently by
// both integration-test crate roots (mirrors the pre-existing
// `tests/support/failing_storage.rs` pattern).

/// A no-op `AuditLogPort` impl used when tests don't need to inspect audit entries.
pub struct NoopAudit;

#[async_trait::async_trait]
impl AuditLogPort for NoopAudit {
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
        Default::default()
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

// ── fixture_bridge / fixture_metrics ─────────────────────────────────────────

use super::super::audit_bridge::AuditBridge;
use super::super::metrics::ExternalMetrics;

/// Capturing `AuditLogPort` impl used by [`fixture_bridge`].
///
/// Construct via [`fixture_bridge`]`() -> (AuditBridge, Arc<MockRecorder>)` — the
/// struct's `new()` constructor is crate-private to enforce this factory pattern.
/// Tests read captured entries via [`MockRecorder::snapshot`].
///
/// Used by `fixture_bridge` so that Tasks 0.6 and 3.1 can assert on the
/// exact audit entries emitted by `AuditBridge::record` /
/// `AuditBridge::record_completion`.
pub struct MockRecorder {
    /// All entries captured by legacy completion and typed audit calls.
    pub entries: parking_lot::Mutex<Vec<AuditEntry>>,
}

impl MockRecorder {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            entries: parking_lot::Mutex::new(vec![]),
        })
    }

    /// Snapshot entries without holding the lock past the call.
    pub fn snapshot(&self) -> Vec<AuditEntry> {
        self.entries.lock().clone()
    }
}

#[async_trait::async_trait]
impl AuditLogPort for MockRecorder {
    async fn pending_count(&self) -> usize {
        0
    }
    async fn recent_entries(&self, _limit: usize) -> Vec<AuditEntry> {
        vec![]
    }
    async fn entries_by_status(&self, _s: &AuditStatus, _l: usize) -> Vec<AuditEntry> {
        vec![]
    }
    async fn entries_by_action_prefix(&self, _p: &str, _l: usize) -> Vec<AuditEntry> {
        vec![]
    }
    async fn entries_by_command_id(&self, _cmd_id: &str, _limit: usize) -> Vec<AuditEntry> {
        vec![]
    }
    async fn stats(&self) -> AuditStats {
        Default::default()
    }
    async fn has_pending_batch(&self) -> bool {
        false
    }
    async fn log_event(&self, _a: &str, _s: &str, _d: &str) {}
    async fn log_start_if(&self, _l: AuditLevel, _c: &str, _s: &str, _a: &str) {}
    async fn log_complete_with_time(
        &self,
        _level: AuditLevel,
        command_id: &str,
        session_id: &str,
        details: &str,
        execution_time_ms: u64,
    ) {
        let status = serde_json::from_str::<serde_json::Value>(details)
            .ok()
            .and_then(|v| {
                v.get("result").and_then(|r| r.as_str()).map(|r| match r {
                    "ok" => AuditStatus::Completed,
                    "denied" => AuditStatus::Denied,
                    "timeout" => AuditStatus::Timeout,
                    _ => AuditStatus::Failed,
                })
            })
            .unwrap_or(AuditStatus::Completed);
        self.log_with_status_and_time(
            _level,
            command_id,
            session_id,
            "external_grpc",
            status,
            details,
            execution_time_ms,
        )
        .await;
    }
    async fn log_with_status_and_time(
        &self,
        _level: AuditLevel,
        command_id: &str,
        session_id: &str,
        action_type: &str,
        status: AuditStatus,
        details: &str,
        execution_time_ms: u64,
    ) {
        self.entries.lock().push(AuditEntry {
            entry_id: ulid::Ulid::generate().to_string(),
            timestamp: chrono::Utc::now(),
            session_id: session_id.into(),
            command_id: command_id.into(),
            action_type: action_type.into(),
            status,
            details: Some(details.into()),
            execution_time_ms: Some(execution_time_ms),
        });
    }
    async fn drain_batch(&self) -> Vec<AuditEntry> {
        vec![]
    }
    async fn drain_all(&self) -> Vec<AuditEntry> {
        vec![]
    }
    async fn record_session_event(&self, _e: SessionAuditEntry) {}
}

/// Build an `(AuditBridge, Arc<MockRecorder>)` pair ready for unit tests.
///
/// The `MockRecorder` captures legacy completion and explicit-status calls; the
/// `AuditBridge` delegates to it. Used by Tasks 0.6 and 3.1.
pub fn fixture_bridge() -> (AuditBridge, Arc<MockRecorder>) {
    let recorder = MockRecorder::new();
    let bridge = AuditBridge::new(recorder.clone() as Arc<dyn AuditLogPort>);
    (bridge, recorder)
}

/// Build a fresh `Arc<ExternalMetrics>` pre-zeroed. Used by Tasks 0.6 and 3.1.
pub fn fixture_metrics() -> Arc<ExternalMetrics> {
    Arc::new(Default::default())
}

#[cfg(test)]
#[path = "audit/tests.rs"]
mod audit_contract_tests;
