use oneshim_core::models::tiered_memory::CalibrationEntry;
use std::time::Instant;

/// In-memory buffer that accumulates `CalibrationEntry` values and signals
/// when a flush is needed — either because the buffer is full or because
/// the flush interval has elapsed.
pub struct CalibrationBuffer {
    entries: Vec<CalibrationEntry>,
    max_size: usize,
    flush_interval_secs: u64,
    last_flush: Instant,
}

impl CalibrationBuffer {
    pub fn new(max_size: usize, flush_interval_secs: u64) -> Self {
        Self {
            entries: Vec::with_capacity(max_size),
            max_size,
            flush_interval_secs,
            last_flush: Instant::now(),
        }
    }

    /// Push an entry into the buffer.
    /// Returns `Some(batch)` when a flush is needed (capacity reached or
    /// interval elapsed), `None` otherwise.
    pub fn push(&mut self, entry: CalibrationEntry) -> Option<Vec<CalibrationEntry>> {
        self.entries.push(entry);

        let capacity_full = self.entries.len() >= self.max_size;
        let interval_elapsed = self.last_flush.elapsed().as_secs() >= self.flush_interval_secs;

        if capacity_full || interval_elapsed {
            Some(self.force_flush())
        } else {
            None
        }
    }

    /// Drain all buffered entries regardless of thresholds.
    pub fn force_flush(&mut self) -> Vec<CalibrationEntry> {
        self.last_flush = Instant::now();
        std::mem::take(&mut self.entries)
    }

    /// Number of entries currently buffered.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use oneshim_core::models::work_session::AppCategory;

    fn make_entry() -> CalibrationEntry {
        CalibrationEntry {
            timestamp: Utc::now(),
            event_type: "APP_SWITCH_NEW".to_string(),
            app_name: "VSCode".to_string(),
            app_category: AppCategory::Development,
            event_importance: 0.5,
            density_signal: 0.3,
            importance_signal: 0.4,
            context_signal: 0.2,
            buffer_signal: 0.1,
            trigger_score: 0.6,
            trigger_action: None,
            active_regime_id: None,
            params_version_id: "v1".to_string(),
            params_json: String::new(),
            is_noise: false,
        }
    }

    #[test]
    fn flush_on_capacity() {
        let mut buf = CalibrationBuffer::new(3, 9999);

        assert!(buf.push(make_entry()).is_none());
        assert!(buf.push(make_entry()).is_none());
        let batch = buf.push(make_entry());
        assert!(batch.is_some());
        assert_eq!(batch.unwrap().len(), 3);
        assert!(buf.is_empty());
    }

    #[test]
    fn flush_on_interval() {
        let mut buf = CalibrationBuffer::new(9999, 0); // 0-second interval → immediate flush

        let batch = buf.push(make_entry());
        assert!(batch.is_some());
        assert_eq!(batch.unwrap().len(), 1);
    }

    #[test]
    fn force_flush_empty() {
        let mut buf = CalibrationBuffer::new(10, 60);
        let batch = buf.force_flush();
        assert!(batch.is_empty());
    }

    #[test]
    fn force_flush_partial() {
        let mut buf = CalibrationBuffer::new(10, 60);
        buf.push(make_entry()); // won't trigger auto-flush
        buf.push(make_entry());
        assert_eq!(buf.len(), 2);

        let batch = buf.force_flush();
        assert_eq!(batch.len(), 2);
        assert!(buf.is_empty());
    }

    #[test]
    fn empty_state() {
        let buf = CalibrationBuffer::new(10, 60);
        assert!(buf.is_empty());
        assert_eq!(buf.len(), 0);
    }
}
