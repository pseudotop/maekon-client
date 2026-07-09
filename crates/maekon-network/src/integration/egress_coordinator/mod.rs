//! IntegrationEgressCoordinator — outbound egress orchestration.
//!
//! ADR-013: 500L threshold applied — original 1197L file converted to module
//! folder with submodules:
//!   budget.rs — MAX_OUTBOX_ITEMS / MAX_OUTBOX_BYTES / byte estimation helpers
//!   tests.rs  — all #[cfg(test)] fixtures and test cases

mod budget;
#[cfg(test)]
mod tests;

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use maekon_core::error::CoreError;
use maekon_core::models::integration::{
    IntegrationAckCursor, IntegrationEnvelope, IntegrationOutboundPayload,
    IntegrationSessionStatus, QueuedIntegrationEgressMessage,
};
use maekon_core::ports::integration::{
    IntegrationEgressPort, IntegrationEgressSignalPort, IntegrationOutboxPort,
    IntegrationSessionPort,
};
use tokio::sync::{Notify, OnceCell};
use tracing::warn;

use super::transport::IntegrationEgressTransportClient;
use budget::{estimate_message_bytes, MAX_OUTBOX_BYTES, MAX_OUTBOX_ITEMS};

pub struct IntegrationEgressCoordinator {
    session_port: Arc<dyn IntegrationSessionPort>,
    outbox: Arc<dyn IntegrationOutboxPort>,
    transport: Arc<dyn IntegrationEgressTransportClient>,
    max_batch_size: usize,
    flush_notify: Arc<Notify>,
    /// F-RR-C25-05: estimated number of bytes currently accumulated in the outbox.
    /// Incremented in `enqueue_message`, decremented once the ack completes after a flush.
    pub(crate) pending_bytes: AtomicUsize,
    /// #6127: one-shot guard ensuring `pending_bytes` is reconciled from the
    /// persistent outbox exactly once before the first enqueue/flush. A freshly
    /// constructed coordinator seeds `pending_bytes` to 0, but the on-disk
    /// outbox may already hold a pre-restart backlog. Without reconciliation,
    /// flushing that backlog would decrement a counter that never counted it,
    /// underflowing the unsigned counter. The reconcile is idempotent and runs
    /// lazily so wiring cannot accidentally skip it.
    reconciled: OnceCell<()>,
}

impl IntegrationEgressCoordinator {
    pub fn new(
        session_port: Arc<dyn IntegrationSessionPort>,
        outbox: Arc<dyn IntegrationOutboxPort>,
        transport: Arc<dyn IntegrationEgressTransportClient>,
        max_batch_size: usize,
    ) -> Self {
        Self {
            session_port,
            outbox,
            transport,
            max_batch_size: max_batch_size.max(1),
            flush_notify: Arc::new(Notify::new()),
            pending_bytes: AtomicUsize::new(0),
            reconciled: OnceCell::new(),
        }
    }

    /// #6127: Seed `pending_bytes` from the full persisted outbox backlog.
    ///
    /// On restart the in-memory `pending_bytes` counter starts at 0, but the
    /// SQLite-backed outbox may still hold messages enqueued before shutdown.
    /// This reconciliation sums the serialized byte estimate over **all**
    /// persisted pending items (not a single `max_batch_size` page) so that the
    /// counter reflects the real on-disk backlog before any flush decrements it.
    ///
    /// Idempotent: the underlying `OnceCell` runs the body at most once, so
    /// calling this during wiring and/or having it triggered lazily on the
    /// first operation both converge to a single reconciliation.
    pub async fn reconcile_pending_bytes(&self) -> Result<(), CoreError> {
        // `get_or_try_init` only flips the cell to initialized when the closure
        // returns `Ok`; a transient outbox read error leaves it un-reconciled
        // so a later call (or the lazy first-op trigger) can retry.
        self.reconciled
            .get_or_try_init(|| async {
                // List the FULL outbox, not one batch: pass the current pending
                // count as the limit so every persisted item is summed.
                let total_items = self.outbox.pending_count().await?;
                let items = self.outbox.list_pending(total_items).await?;
                let backlog_bytes: usize = items
                    .iter()
                    .map(|item| {
                        serde_json::to_vec(&item.envelope)
                            .map(|v| v.len())
                            .unwrap_or(0)
                            + serde_json::to_vec(&item.payload)
                                .map(|v| v.len())
                                .unwrap_or(0)
                    })
                    .sum();
                self.pending_bytes.store(backlog_bytes, Ordering::Relaxed);
                Ok::<(), CoreError>(())
            })
            .await
            .map(|_| ())
    }

    /// Returns the current estimated accumulated byte count (for monitoring/testing).
    ///
    /// # Caution: Relaxed-ordering snapshot
    ///
    /// This value is read with `Ordering::Relaxed`, so it is a best-effort
    /// approximation. Its ordering with respect to concurrent `enqueue_message`
    /// / `flush` calls is not guaranteed, so depending on the moment it is
    /// called it may reflect either the state just before or just after them.
    ///
    /// Using the returned value as a **strict upper-bound** (e.g.
    /// `assert!(v <= MAX)`) can therefore trigger non-deterministic test
    /// failures. Use it only for best-effort purposes such as monotonic peak
    /// tracking (see the F-RR-C26-01 test).
    pub fn pending_bytes(&self) -> usize {
        self.pending_bytes.load(Ordering::Relaxed)
    }

    fn validate_scopes(
        session: &maekon_core::models::integration::IntegrationSessionState,
        items: &[QueuedIntegrationEgressMessage],
    ) -> Result<(), CoreError> {
        let missing_scope = items.iter().find_map(|item| {
            (!session
                .granted_scopes
                .contains(&item.envelope.capability_scope))
            .then_some(item.envelope.capability_scope.clone())
        });

        if let Some(scope) = missing_scope {
            return Err(CoreError::Auth {
                code: maekon_core::error_codes::AuthCode::Failed,
                message: format!("integration session is missing required scope: {scope:?}"),
            });
        }

        Ok(())
    }

    fn acknowledged_queue_ids(
        sent_items: &[QueuedIntegrationEgressMessage],
        response: &super::transport::IntegrationEgressTransportResponse,
    ) -> Result<Vec<String>, CoreError> {
        let sent_ids: BTreeSet<&str> = sent_items
            .iter()
            .map(|item| item.queue_id.as_str())
            .collect();
        if let Some(unknown_id) = response
            .acknowledged_queue_ids
            .iter()
            .find(|queue_id| !sent_ids.contains(queue_id.as_str()))
        {
            return Err(CoreError::Internal {
                code: maekon_core::error_codes::InternalCode::Generic,
                message: format!(
                    "integration egress transport acknowledged unknown queue id: {unknown_id}"
                ),
            });
        }

        Ok(response.acknowledged_queue_ids.clone())
    }
}

#[async_trait]
impl IntegrationEgressSignalPort for IntegrationEgressCoordinator {
    async fn wait_for_pending_egress(&self, timeout: Duration) -> Result<bool, CoreError> {
        // #7617 (LOW finding #6 / RL-02): `Notify::notify_one()` stores a
        // single wake-up permit even when called before anyone is waiting,
        // so a `flush_notify.notify_one()` raised while the runtime loop is
        // mid-await in a SIBLING `select!` arm (connect/heartbeat/inbox) is
        // not discarded -- `.notified()` consumes the stored permit and
        // returns immediately the next time it is awaited. This crate's
        // runtime loop is a single-consumer per coordinator instance, so a
        // stored single permit is exactly the right semantics (unlike
        // `notify_waiters()`, which only wakes tasks ALREADY waiting and
        // stores nothing for a future call). See `enqueue_message` below for
        // the producer side.
        match tokio::time::timeout(timeout, self.flush_notify.notified()).await {
            Ok(_) => Ok(true),
            Err(_) => Ok(false),
        }
    }
}

#[async_trait]
impl IntegrationEgressPort for IntegrationEgressCoordinator {
    async fn enqueue_message(
        &self,
        envelope: IntegrationEnvelope,
        payload: IntegrationOutboundPayload,
    ) -> Result<(), CoreError> {
        // #6127: reconcile the byte counter from the persisted backlog before
        // the first enqueue, so a pre-restart outbox is accounted for.
        self.reconcile_pending_bytes().await?;

        // F-PF-C24-05: count cap — reject once the item count reaches MAX_OUTBOX_ITEMS.
        let count = self.outbox.pending_count().await?;
        if count >= MAX_OUTBOX_ITEMS {
            return Err(CoreError::ServiceUnavailable {
                code: maekon_core::error_codes::ServiceCode::Unavailable,
                message: format!(
                    "integration outbox full ({MAX_OUTBOX_ITEMS} items pending); \
                     message dropped until flush drains the queue"
                ),
            });
        }

        // F-RR-C26-01: atomic reserve-then-check pattern.
        let msg_bytes = estimate_message_bytes(&envelope, &payload);
        let new_total = self.pending_bytes.fetch_add(msg_bytes, Ordering::Relaxed) + msg_bytes;
        if new_total > MAX_OUTBOX_BYTES {
            self.pending_bytes.fetch_sub(msg_bytes, Ordering::Relaxed);
            warn!(
                new_total,
                msg_bytes,
                max_bytes = MAX_OUTBOX_BYTES,
                "outbox byte cap exceeded — message rejected (retry after flush)"
            );
            return Err(CoreError::ServiceUnavailable {
                code: maekon_core::error_codes::ServiceCode::Unavailable,
                message: format!(
                    "integration outbox byte cap exceeded \
                     ({new_total} bytes after reservation > {MAX_OUTBOX_BYTES} limit); \
                     message dropped until flush drains the queue"
                ),
            });
        }

        match self.outbox.enqueue_message(envelope, payload).await {
            Ok(_queue_id) => {
                // #7617 (LOW finding #6 / RL-02): use `notify_one()` (stores a
                // permit) instead of `notify_waiters()` (wakes only tasks
                // already parked in `.notified()`, storing nothing). The
                // runtime loop's `select!` future set is re-created every
                // loop iteration, so a `notify_waiters()` raised while the
                // loop is mid-await in a sibling arm (connect/heartbeat/
                // inbox) was silently discarded -- the queued item then had
                // to wait for the next periodic egress tick (up to 4x the
                // base interval under the #6516 quiet-backoff) instead of
                // being flushed promptly. `notify_one()` is exactly the right
                // primitive for this single-consumer-per-coordinator loop: at
                // most one permit is ever meaningful since there is only one
                // `wait_for_pending_egress` caller.
                self.flush_notify.notify_one();
                Ok(())
            }
            Err(e) => {
                self.pending_bytes.fetch_sub(msg_bytes, Ordering::Relaxed);
                Err(e)
            }
        }
    }

    async fn flush(&self) -> Result<usize, CoreError> {
        // #6127: reconcile the byte counter from the persisted backlog before
        // the first flush, so flushing a pre-restart backlog decrements a
        // counter that actually accounted for it (no underflow).
        self.reconcile_pending_bytes().await?;

        let session = self.session_port.current_session().await?.ok_or_else(|| {
            CoreError::ServiceUnavailable {
                code: maekon_core::error_codes::ServiceCode::Unavailable,
                message: "integration session is not connected".to_string(),
            }
        })?;

        if !matches!(
            session.status,
            IntegrationSessionStatus::Connected | IntegrationSessionStatus::Degraded
        ) || session.session_id.is_empty()
        {
            return Err(CoreError::ServiceUnavailable {
                code: maekon_core::error_codes::ServiceCode::Unavailable,
                message: "integration session is not ready for outbound egress".to_string(),
            });
        }

        let items = self.outbox.list_pending(self.max_batch_size).await?;
        if items.is_empty() {
            return Ok(0);
        }
        Self::validate_scopes(&session, &items)?;

        let response = self
            .transport
            .send_messages(&session.session_id, items.clone())
            .await?;
        let acknowledged_queue_ids = Self::acknowledged_queue_ids(&items, &response)?;
        let accepted_count = response.accepted_count();

        if !acknowledged_queue_ids.is_empty() {
            let acked_items: Vec<&QueuedIntegrationEgressMessage> = items
                .iter()
                .filter(|item| acknowledged_queue_ids.contains(&item.queue_id))
                .collect();
            let acked_bytes: usize = acked_items
                .iter()
                .map(|item| {
                    serde_json::to_vec(&item.envelope)
                        .map(|v| v.len())
                        .unwrap_or(0)
                        + serde_json::to_vec(&item.payload)
                            .map(|v| v.len())
                            .unwrap_or(0)
                })
                .sum();
            // F-RR-C27-01/F-RC-C27-01: race-free decrement.
            // #6127: saturating CAS loop clamping at 0 — decrementing more than
            // the tracked bytes (e.g. flushing an unreconciled pre-restart
            // backlog) must NOT underflow the unsigned counter.
            let _ =
                self.pending_bytes
                    .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                        Some(current.saturating_sub(acked_bytes))
                    });

            self.outbox.delete(&acknowledged_queue_ids).await?;
        }
        if let Some(cursor) = response.ack_cursor {
            self.outbox.store_ack_cursor(cursor.clone()).await?;
            self.session_port
                .store_ack_cursor(&session.session_id, cursor)
                .await?;
        }

        Ok(accepted_count)
    }

    async fn last_ack_cursor(&self) -> Result<Option<IntegrationAckCursor>, CoreError> {
        self.outbox.last_ack_cursor().await
    }
}
