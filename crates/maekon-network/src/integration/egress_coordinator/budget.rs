//! Outbox budget constants and byte estimation helpers.
//!
//! Separated from the coordinator logic to make the cap values easy to locate
//! and tune independently of the flush/enqueue implementation.

use maekon_core::models::integration::{IntegrationEnvelope, IntegrationOutboundPayload};

/// F-PF-C24-05: upper bound on queued outbound messages (count).  Prevents
/// unbounded outbox growth during extended connectivity loss.  Callers receive
/// `CoreError::ServiceUnavailable` when the cap is reached.
pub(super) const MAX_OUTBOX_ITEMS: usize = 512;

/// F-RR-C25-05: byte-level cap on the outbox to prevent oversized request
/// bodies from OCR-carrying batches.  10 MiB is generous enough for 512 normal
/// insight summaries while bounding worst-case memory/HTTP body size.
pub(super) const MAX_OUTBOX_BYTES: usize = 10 * 1024 * 1024; // 10 MiB

/// Estimates the serialized byte size of a single message.
///
/// Uses `serde_json::to_vec`, so it matches the actual transmitted size exactly.
/// On serialization failure it returns a conservative estimate (64 KiB) to
/// prevent the cap from being bypassed.
pub(super) fn estimate_message_bytes(
    envelope: &IntegrationEnvelope,
    payload: &IntegrationOutboundPayload,
) -> usize {
    let env_bytes = serde_json::to_vec(envelope)
        .map(|v| v.len())
        .unwrap_or(65_536);
    let pay_bytes = serde_json::to_vec(payload)
        .map(|v| v.len())
        .unwrap_or(65_536);
    env_bytes + pay_bytes
}

/// Computes the cumulative byte size of a list of messages already in the queue.
/// Used to decrement the `pending_bytes` counter by the amount of items acked
/// after a flush.
/// F-RR-C27-02: test-only function — prevents the dead_code warning in non-test builds.
#[cfg(test)]
pub(super) fn estimate_queued_bytes(
    items: &[maekon_core::models::integration::QueuedIntegrationEgressMessage],
) -> usize {
    items
        .iter()
        .map(|item| {
            let env = serde_json::to_vec(&item.envelope)
                .map(|v| v.len())
                .unwrap_or(65_536);
            let pay = serde_json::to_vec(&item.payload)
                .map(|v| v.len())
                .unwrap_or(65_536);
            env + pay
        })
        .sum()
}
