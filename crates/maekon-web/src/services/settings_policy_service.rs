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
/// handle.  Designed to be long-lived (one instance per web server, stored in
/// `AppState.automation.policy_audit_writer`) and shared by reference across
/// every per-request `SettingsWebContext` clone.
///
/// Dropping `PolicyAuditWriter` aborts the writer task immediately via
/// `JoinHandle::abort()`.  Because the writer lives for the whole server
/// lifetime (behind an `Arc` in `AppState`), this drop only runs at shutdown —
/// NOT at the end of each request.  That is the crux of the #6117 fix: the
/// previous design built and dropped the writer per request, aborting the
/// background drain task before it could flush the just-enqueued audit event,
/// so security-policy audit events were deterministically lost on every save.
///
/// F-RR-38: the original design constructed a new `(mpsc::channel, tokio::spawn)`
/// on every `emit_policy_change_events` call.  Rapid saves produced N concurrent
/// writer tasks with no upper bound and no cleanup.  #6117 completes the fix by
/// hoisting the single writer all the way up to `AppState`, so one writer task
/// is created at server construction and shared by every request.
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

    /// #6117: deterministically enqueue an audit event into the long-lived
    /// channel, awaiting the send so a momentarily-full channel applies
    /// backpressure on the save path instead of silently dropping the event
    /// (the previous `try_send` behavior).  The drain task that empties the
    /// channel runs for the whole server lifetime, so the awaited send is the
    /// durable hand-off — the event is guaranteed to be queued before
    /// `update_settings` returns its HTTP response.
    ///
    /// Only the closed-channel case (writer task gone) is dropped, which can
    /// only happen at shutdown; settings-policy audit writes are best-effort and
    /// must never block the request indefinitely on a dead writer.
    pub(crate) async fn send(&self, action_type: String, details: String) {
        if let Err(_e) = self
            .tx
            .send(AuditEntry {
                action_type,
                details,
            })
            .await
        {
            warn!("audit channel closed — dropping policy audit event (writer task gone)");
        }
    }

    /// Best-effort, non-blocking enqueue.  Retained for the bounded-drop unit
    /// test; the production save path uses the awaited [`send`](Self::send).
    #[cfg(test)]
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
/// `previous` and `next`.  Uses the provided long-lived `writer` to enqueue
/// events into the durable bounded channel.
///
/// #6117: this is `async` and awaits each enqueue so the audit event is
/// guaranteed to be handed off to the (server-lifetime) drain task before the
/// caller returns — no fire-and-forget into a task that is dropped at request
/// end.  No-op when `writer` is `None`.
pub(crate) async fn emit_policy_change_events(
    writer: Option<&PolicyAuditWriter>,
    previous: &AppConfig,
    next: &AppConfig,
) {
    let Some(writer) = writer else {
        return;
    };

    if previous.ai_provider.bypass_pii_filter_for_external_ocr
        != next.ai_provider.bypass_pii_filter_for_external_ocr
    {
        writer
            .send(
                "policy.settings.bypass_pii_filter_for_external_ocr.changed".to_string(),
                format!(
                    "from={} to={}",
                    previous.ai_provider.bypass_pii_filter_for_external_ocr,
                    next.ai_provider.bypass_pii_filter_for_external_ocr
                ),
            )
            .await;
    }

    let prev_override = &previous.ai_provider.scene_action_override;
    let next_override = &next.ai_provider.scene_action_override;
    let override_changed = prev_override.enabled != next_override.enabled
        || prev_override.reason != next_override.reason
        || prev_override.approved_by != next_override.approved_by
        || prev_override.expires_at != next_override.expires_at;

    if override_changed {
        writer
            .send(
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
            )
            .await;
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
        writer
            .send(
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
            )
            .await;
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

    /// #6117 regression: a SHARED, long-lived `PolicyAuditWriter` delivers the
    /// audit event for a settings save even after the per-request emitter scope
    /// (the `SettingsUpdateFlow` borrow) ends.
    ///
    /// The pre-fix bug: the writer was constructed and dropped *per request*, so
    /// its drain task was `abort()`ed before the fire-and-forget enqueue could
    /// flush — every settings save lost its security-policy audit event.  This
    /// test models the fixed wiring: the writer is owned by an outer (AppState-
    /// like) `Arc`, the per-request emit borrows it and awaits the enqueue, and
    /// after the per-request reference is dropped the event is STILL delivered
    /// because the shared writer's drain task is still alive.
    #[tokio::test]
    async fn shared_writer_delivers_after_request_scope_ends() {
        use maekon_core::config::AppConfig;

        let count = Arc::new(AtomicUsize::new(0));
        let logger = Arc::new(CountingAuditLogger {
            count: count.clone(),
        });
        // Server-lifetime writer (analogous to AppState.automation.policy_audit_writer).
        let shared_writer = Arc::new(PolicyAuditWriter::new(logger));

        let previous = AppConfig::default_config();
        let mut next = previous.clone();
        // Flip a policy-relevant field so exactly one audit event is emitted.
        next.ai_provider.bypass_pii_filter_for_external_ocr =
            !previous.ai_provider.bypass_pii_filter_for_external_ocr;

        {
            // Per-request borrow of the shared writer (what SettingsUpdateFlow does).
            let per_request = Arc::clone(&shared_writer);
            emit_policy_change_events(Some(per_request.as_ref()), &previous, &next).await;
            // The per-request reference is dropped at the end of this block — in
            // the buggy design THIS is where the writer (and its drain task) used
            // to die before flushing.  Here it is only a clone, so the writer
            // task survives.
        }

        // Let the still-alive drain task process the queued event.
        sleep(Duration::from_millis(200)).await;
        assert_eq!(
            count.load(Ordering::SeqCst),
            1,
            "#6117: the shared writer must deliver the audit event even after the per-request scope ends"
        );
    }
}
