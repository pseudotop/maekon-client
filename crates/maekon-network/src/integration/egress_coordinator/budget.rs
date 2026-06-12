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

/// 단일 메시지의 직렬화 바이트 크기를 추정합니다.
///
/// `serde_json::to_vec` 을 사용하므로 실제 전송 크기와 정확하게 일치합니다.
/// 직렬화 실패 시 보수적 추정값(64 KiB)을 반환하여 cap 우회를 방지합니다.
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

/// 이미 큐에 있는 메시지 목록의 누적 바이트 크기를 계산합니다.
/// flush 후 ack된 항목 만큼 pending_bytes 카운터를 감소시킬 때 사용합니다.
/// F-RR-C27-02: 테스트 전용 함수 — non-test 빌드에서 dead_code 경고 방지.
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
