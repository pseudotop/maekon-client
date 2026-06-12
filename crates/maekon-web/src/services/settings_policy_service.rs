use maekon_core::config::AppConfig;
use maekon_core::ports::audit_log::AuditLogPort;
use std::sync::Arc;
use tracing::warn;

/// F-RR-32: bounded audit-write channel capacity.
///
/// `log_policy_event` previously used unbounded `tokio::spawn` for each audit
/// event, which allows an unlimited number of tasks to accumulate if the audit
/// backend is slow.  We replace that pattern with a bounded `mpsc` channel
/// (capacity = 64) and a single persistent writer task.  Events that arrive
/// when the channel is full are dropped with a warning — this is intentional:
/// audit log writes for settings policy changes are best-effort and must never
/// block the API response path.
const AUDIT_CHANNEL_CAPACITY: usize = 64;

/// A lightweight guard that owns the bounded mpsc sender and the writer task
/// handle.  Designed to be long-lived (one instance per `SettingsUpdateFlow`)
/// and reused across multiple `emit_policy_change_events` calls.
///
/// Dropping `PolicyAuditWriter` aborts the writer task immediately via
/// `JoinHandle::abort()`.  Any events still in the channel are discarded —
/// audit log writes for settings policy changes are best-effort.
///
/// F-RR-38: the previous design constructed a new `(mpsc::channel, tokio::spawn)`
/// on every `emit_policy_change_events` call.  Rapid saves produced N concurrent
/// writer tasks with no upper bound and no cleanup.  The fix promotes
/// `PolicyAuditWriter` to a long-lived field on `SettingsUpdateFlow`, so one
/// writer task is created per service instance and reused for all saves.
pub(crate) struct PolicyAuditWriter {
    tx: tokio::sync::mpsc::Sender<AuditEntry>,
    /// The writer task handle.  Aborted in `Drop` to release resources
    /// immediately.  Dropping a `JoinHandle` merely detaches the task;
    /// explicit `abort()` is required for prompt cancellation.
    handle: tokio::task::JoinHandle<()>,
}

struct AuditEntry {
    action_type: String,
    details: String,
}

impl PolicyAuditWriter {
    pub(crate) fn new(audit_logger: Arc<dyn AuditLogPort>) -> Self {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<AuditEntry>(AUDIT_CHANNEL_CAPACITY);
        let handle = tokio::spawn(async move {
            while let Some(entry) = rx.recv().await {
                audit_logger
                    .log_event(&entry.action_type, "settings", &entry.details)
                    .await;
            }
        });
        Self { tx, handle }
    }

    pub(crate) fn try_send(&self, action_type: String, details: String) {
        if let Err(_e) = self.tx.try_send(AuditEntry {
            action_type,
            details,
        }) {
            warn!(
                "audit channel full or closed — dropping policy audit event (F-RR-32 bounded drop)"
            );
        }
    }
}

impl Drop for PolicyAuditWriter {
    fn drop(&mut self) {
        // Abort the writer task promptly.  Dropping a JoinHandle only detaches
        // the task; without an explicit abort() the writer would keep running
        // until the sender side is also dropped (which it is, here), but
        // `abort()` makes the cleanup intent explicit and deterministic.
        self.handle.abort();
    }
}

/// Emit audit events for every policy-relevant field that changed between
/// `previous` and `next`.  Uses the provided `writer` to queue events into
/// the bounded channel.  The writer should be long-lived (see F-RR-38).
///
/// No-op when `writer` is `None`.
pub(crate) fn emit_policy_change_events(
    writer: Option<&PolicyAuditWriter>,
    previous: &AppConfig,
    next: &AppConfig,
) {
    let Some(writer) = writer else {
        return;
    };

    if previous.ai_provider.allow_unredacted_external_ocr
        != next.ai_provider.allow_unredacted_external_ocr
    {
        writer.try_send(
            "policy.settings.allow_unredacted_external_ocr.changed".to_string(),
            format!(
                "from={} to={}",
                previous.ai_provider.allow_unredacted_external_ocr,
                next.ai_provider.allow_unredacted_external_ocr
            ),
        );
    }

    let prev_override = &previous.ai_provider.scene_action_override;
    let next_override = &next.ai_provider.scene_action_override;
    let override_changed = prev_override.enabled != next_override.enabled
        || prev_override.reason != next_override.reason
        || prev_override.approved_by != next_override.approved_by
        || prev_override.expires_at != next_override.expires_at;

    if override_changed {
        writer.try_send(
            "policy.settings.scene_action_override.changed".to_string(),
            format!(
                "from_enabled={} to_enabled={} from_reason={:?} to_reason={:?} from_approved_by={:?} to_approved_by={:?} from_expires_at={:?} to_expires_at={:?}",
                prev_override.enabled,
                next_override.enabled,
                prev_override.reason.as_deref(),
                next_override.reason.as_deref(),
                prev_override.approved_by.as_deref(),
                next_override.approved_by.as_deref(),
                prev_override.expires_at.map(|value| value.to_rfc3339()),
                next_override.expires_at.map(|value| value.to_rfc3339()),
            ),
        );
    }

    let prev_scene = &previous.ai_provider.scene_intelligence;
    let next_scene = &next.ai_provider.scene_intelligence;
    let scene_changed = prev_scene.enabled != next_scene.enabled
        || prev_scene.overlay_enabled != next_scene.overlay_enabled
        || prev_scene.allow_action_execution != next_scene.allow_action_execution
        || (prev_scene.min_confidence - next_scene.min_confidence).abs() > f64::EPSILON
        || prev_scene.max_elements != next_scene.max_elements
        || prev_scene.calibration_enabled != next_scene.calibration_enabled
        || prev_scene.calibration_min_elements != next_scene.calibration_min_elements
        || (prev_scene.calibration_min_avg_confidence - next_scene.calibration_min_avg_confidence)
            .abs()
            > f64::EPSILON;

    if scene_changed {
        writer.try_send(
            "policy.settings.scene_intelligence.changed".to_string(),
            format!(
                "enabled {}->{} overlay {}->{} allow_action_execution {}->{} min_confidence {:.2}->{:.2} max_elements {}->{} calibration_enabled {}->{} calibration_min_elements {}->{} calibration_min_avg_confidence {:.2}->{:.2}",
                prev_scene.enabled,
                next_scene.enabled,
                prev_scene.overlay_enabled,
                next_scene.overlay_enabled,
                prev_scene.allow_action_execution,
                next_scene.allow_action_execution,
                prev_scene.min_confidence,
                next_scene.min_confidence,
                prev_scene.max_elements,
                next_scene.max_elements,
                prev_scene.calibration_enabled,
                next_scene.calibration_enabled,
                prev_scene.calibration_min_elements,
                next_scene.calibration_min_elements,
                prev_scene.calibration_min_avg_confidence,
                next_scene.calibration_min_avg_confidence,
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maekon_core::models::audit::{AuditEntry, AuditLevel, AuditStats, AuditStatus};
    use maekon_core::ports::audit_log::AuditLogPort;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::time::{sleep, Duration};

    struct CountingAuditLogger {
        count: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl AuditLogPort for CountingAuditLogger {
        async fn log_event(&self, _action_type: &str, _resource: &str, _details: &str) {
            self.count.fetch_add(1, Ordering::SeqCst);
        }
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
        async fn stats(&self) -> AuditStats {
            AuditStats::default()
        }
        async fn has_pending_batch(&self) -> bool {
            false
        }
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
        async fn entries_by_command_id(&self, _command_id: &str, _limit: usize) -> Vec<AuditEntry> {
            vec![]
        }
    }

    /// F-RR-38: single writer instance handles events without spawning
    /// additional tasks.  Sends exactly AUDIT_CHANNEL_CAPACITY events (fits
    /// within the bounded channel) and asserts all are delivered through one
    /// persistent channel — no per-call tokio::spawn.
    #[tokio::test]
    async fn single_writer_handles_rapid_events() {
        let count = Arc::new(AtomicUsize::new(0));
        let logger = Arc::new(CountingAuditLogger {
            count: count.clone(),
        });
        let writer = PolicyAuditWriter::new(logger);

        // Send exactly AUDIT_CHANNEL_CAPACITY events — fits in the channel
        // without triggering the bounded-drop path.
        for i in 0..AUDIT_CHANNEL_CAPACITY {
            writer.try_send(format!("test.event.{i}"), format!("details {i}"));
        }

        // Give the single writer task time to drain the channel.
        sleep(Duration::from_millis(200)).await;
        assert_eq!(
            count.load(Ordering::SeqCst),
            AUDIT_CHANNEL_CAPACITY,
            "F-RR-38: single writer should have processed all {} events",
            AUDIT_CHANNEL_CAPACITY
        );
    }

    /// F-RR-38: Drop aborts the writer task explicitly.
    #[tokio::test]
    async fn drop_aborts_writer_handle() {
        let count = Arc::new(AtomicUsize::new(0));
        let logger = Arc::new(CountingAuditLogger {
            count: count.clone(),
        });
        let writer = PolicyAuditWriter::new(logger);
        // Drop immediately — handle.abort() should be called.
        drop(writer);
        // No assertion needed beyond "does not panic or hang".
    }
}
