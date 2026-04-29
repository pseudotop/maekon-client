use oneshim_vision::ring_buffer::CaptureRingBuffer;
use tracing::warn;

/// Log ring buffer evictions since last check. Called once per monitor tick.
pub(crate) fn log_ring_buffer_evictions(ring_buffer: &CaptureRingBuffer) {
    let evicted = ring_buffer.take_evicted_count();
    if evicted > 0 {
        warn!(
            count = evicted,
            "ring buffer evictions since last check — capture rate may exceed buffer capacity"
        );
    }
}
