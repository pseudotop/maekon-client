//! Dashcam-style ring buffer for frame capture.
//!
//! Continuously records lightweight thumbnail frames into a fixed-size circular
//! buffer. When a significant event occurs (importance >= threshold), the buffer
//! is flushed to provide "before" context, and a post-event counter is set to
//! continue capturing "after" frames.

use chrono::{DateTime, Utc};
use oneshim_core::models::focused_element::AccessibilityElement;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;
use tracing::{debug, error};

/// A lightweight frame stored in the ring buffer (thumbnail only).
#[derive(Debug, Clone)]
pub struct RingFrame {
    pub timestamp: DateTime<Utc>,
    pub thumbnail_data: Vec<u8>,
    pub app_name: String,
    pub window_title: String,
    /// Accessibility tree snapshot at capture time (empty if unavailable).
    pub accessibility_elements: Vec<AccessibilityElement>,
}

/// Result of flushing the ring buffer on a significant event.
#[derive(Debug)]
pub struct RingBufferFlush {
    /// Pre-event frames (oldest first).
    pub pre_event_frames: Vec<RingFrame>,
    /// The trigger frame that caused the flush.
    pub trigger_frame: RingFrame,
}

/// Dashcam-style ring buffer that retains recent frames for context.
pub struct CaptureRingBuffer {
    buffer: Mutex<VecDeque<RingFrame>>,
    capacity: usize,
    /// Remaining post-event frames to force-capture after a significant event.
    post_event_remaining: AtomicU32,
    /// Number of post-event frames to capture after each flush.
    post_event_count: u32,
    /// Minimum importance threshold to trigger a flush.
    flush_threshold: f32,
    /// Cumulative count of frames evicted due to buffer being full.
    evicted_count: AtomicU64,
}

impl CaptureRingBuffer {
    /// Create a new ring buffer.
    ///
    /// - `capacity`: max frames to retain (e.g. 6 for ~18s at 3s intervals)
    /// - `post_event_count`: frames to force-capture after a flush (e.g. 2-3)
    /// - `flush_threshold`: minimum importance to trigger flush (e.g. 0.5)
    pub fn new(capacity: usize, post_event_count: u32, flush_threshold: f32) -> Self {
        Self {
            buffer: Mutex::new(VecDeque::with_capacity(capacity)),
            capacity,
            post_event_remaining: AtomicU32::new(0),
            post_event_count,
            flush_threshold,
            evicted_count: AtomicU64::new(0),
        }
    }

    /// Push a thumbnail frame into the ring buffer (circular overwrite).
    pub fn push(&self, frame: RingFrame) {
        let Ok(mut buf) = self.buffer.lock() else {
            error!("CaptureRingBuffer lock poisoned");
            return;
        };
        if buf.len() >= self.capacity {
            if let Some(evicted) = buf.pop_front() {
                debug!(
                    evicted_ts = %evicted.timestamp,
                    app = %evicted.app_name,
                    buffer_cap = self.capacity,
                    "ring buffer full — evicted oldest frame"
                );
            }
            self.evicted_count.fetch_add(1, Ordering::Relaxed);
        }
        buf.push_back(frame);
    }

    /// Check if the given importance warrants a flush, and if so, drain the
    /// buffer and return all pre-event frames plus the current trigger frame.
    ///
    /// Also sets the post-event counter so subsequent calls to
    /// `should_force_capture()` return true for the next N cycles.
    pub fn check_and_flush(
        &self,
        importance: f32,
        trigger_frame: RingFrame,
    ) -> Option<RingBufferFlush> {
        if importance < self.flush_threshold {
            return None;
        }

        let pre_event_frames = {
            let Ok(mut buf) = self.buffer.lock() else {
                error!("CaptureRingBuffer lock poisoned on flush");
                return None;
            };
            let frames: Vec<RingFrame> = buf.drain(..).collect();
            frames
        };

        // Activate post-event capture
        self.post_event_remaining
            .store(self.post_event_count, Ordering::Relaxed);

        debug!(
            "ring buffer flush: {} pre-event frames, post-event={} remaining",
            pre_event_frames.len(),
            self.post_event_count
        );

        Some(RingBufferFlush {
            pre_event_frames,
            trigger_frame,
        })
    }

    /// Returns true if we're in the post-event capture window.
    /// Each call decrements the counter by 1.
    pub fn should_force_post_capture(&self) -> bool {
        let remaining = self.post_event_remaining.load(Ordering::Relaxed);
        if remaining > 0 {
            self.post_event_remaining.fetch_sub(1, Ordering::Relaxed);
            true
        } else {
            false
        }
    }

    /// Current number of frames in the buffer.
    pub fn len(&self) -> usize {
        self.buffer.lock().map(|b| b.len()).unwrap_or(0)
    }

    /// Whether the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Current post-event remaining count (for diagnostics).
    pub fn post_event_remaining(&self) -> u32 {
        self.post_event_remaining.load(Ordering::Relaxed)
    }

    /// Return the cumulative eviction count without resetting it.
    pub fn evicted_count(&self) -> u64 {
        self.evicted_count.load(Ordering::Relaxed)
    }

    /// Atomically read and reset the eviction counter (swap to 0).
    /// Useful for periodic metric reporting in the scheduler.
    pub fn take_evicted_count(&self) -> u64 {
        self.evicted_count.swap(0, Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_frame(app: &str, title: &str) -> RingFrame {
        RingFrame {
            timestamp: Utc::now(),
            thumbnail_data: vec![0u8; 100],
            app_name: app.to_string(),
            window_title: title.to_string(),
            accessibility_elements: Vec::new(),
        }
    }

    #[test]
    fn push_within_capacity() {
        let rb = CaptureRingBuffer::new(5, 2, 0.5);
        for i in 0..3 {
            rb.push(make_frame("app", &format!("title-{i}")));
        }
        assert_eq!(rb.len(), 3);
    }

    #[test]
    fn push_overwrites_oldest_when_full() {
        let rb = CaptureRingBuffer::new(3, 2, 0.5);
        for i in 0..5 {
            rb.push(make_frame("app", &format!("title-{i}")));
        }
        assert_eq!(rb.len(), 3);

        // Oldest should be title-2 (0 and 1 evicted)
        let buf = rb.buffer.lock().unwrap();
        assert_eq!(buf[0].window_title, "title-2");
        assert_eq!(buf[2].window_title, "title-4");
    }

    #[test]
    fn flush_below_threshold_returns_none() {
        let rb = CaptureRingBuffer::new(5, 2, 0.5);
        rb.push(make_frame("app", "a"));
        rb.push(make_frame("app", "b"));

        let result = rb.check_and_flush(0.3, make_frame("app", "trigger"));
        assert!(result.is_none());
        assert_eq!(rb.len(), 2); // buffer unchanged
    }

    #[test]
    fn flush_above_threshold_drains_buffer() {
        let rb = CaptureRingBuffer::new(5, 2, 0.5);
        rb.push(make_frame("app", "before-1"));
        rb.push(make_frame("app", "before-2"));

        let result = rb.check_and_flush(0.7, make_frame("app", "trigger"));
        assert!(result.is_some());

        let flush = result.unwrap();
        assert_eq!(flush.pre_event_frames.len(), 2);
        assert_eq!(flush.pre_event_frames[0].window_title, "before-1");
        assert_eq!(flush.pre_event_frames[1].window_title, "before-2");
        assert_eq!(flush.trigger_frame.window_title, "trigger");

        // Buffer should be empty after flush
        assert!(rb.is_empty());
    }

    #[test]
    fn post_event_counter_activates_after_flush() {
        let rb = CaptureRingBuffer::new(5, 3, 0.5);
        rb.check_and_flush(0.8, make_frame("app", "trigger"));

        assert!(rb.should_force_post_capture()); // 3 → 2
        assert!(rb.should_force_post_capture()); // 2 → 1
        assert!(rb.should_force_post_capture()); // 1 → 0
        assert!(!rb.should_force_post_capture()); // 0, no more
    }

    #[test]
    fn post_event_remaining_diagnostic() {
        let rb = CaptureRingBuffer::new(5, 2, 0.5);
        assert_eq!(rb.post_event_remaining(), 0);

        rb.check_and_flush(0.6, make_frame("app", "t"));
        assert_eq!(rb.post_event_remaining(), 2);

        rb.should_force_post_capture();
        assert_eq!(rb.post_event_remaining(), 1);
    }

    #[test]
    fn empty_buffer_flush_returns_empty_pre_event() {
        let rb = CaptureRingBuffer::new(5, 2, 0.5);

        let result = rb.check_and_flush(0.9, make_frame("app", "trigger"));
        assert!(result.is_some());
        assert!(result.unwrap().pre_event_frames.is_empty());
    }

    #[test]
    fn push_frame_with_accessibility_elements() {
        use oneshim_core::models::focused_element::{AccessibilityElement, ElementRect};

        let rb = CaptureRingBuffer::new(5, 2, 0.5);
        let frame = RingFrame {
            timestamp: Utc::now(),
            thumbnail_data: vec![0u8; 50],
            app_name: "VSCode".to_string(),
            window_title: "main.rs".to_string(),
            accessibility_elements: vec![
                AccessibilityElement {
                    role: "Editor".to_string(),
                    label: "main.rs".to_string(),
                    bounds: Some(ElementRect {
                        x: 0.0,
                        y: 0.0,
                        width: 1200.0,
                        height: 800.0,
                    }),
                },
                AccessibilityElement {
                    role: "Tab".to_string(),
                    label: "main.rs".to_string(),
                    bounds: None,
                },
            ],
        };
        rb.push(frame);
        assert_eq!(rb.len(), 1);

        let buf = rb.buffer.lock().unwrap();
        assert_eq!(buf[0].accessibility_elements.len(), 2);
        assert_eq!(buf[0].accessibility_elements[0].role, "Editor");
    }

    #[test]
    fn multiple_flushes_work_independently() {
        let rb = CaptureRingBuffer::new(5, 1, 0.5);

        // First flush
        rb.push(make_frame("app", "a"));
        let flush1 = rb.check_and_flush(0.6, make_frame("app", "t1")).unwrap();
        assert_eq!(flush1.pre_event_frames.len(), 1);

        // Accumulate more, second flush
        rb.push(make_frame("app", "b"));
        rb.push(make_frame("app", "c"));
        let flush2 = rb.check_and_flush(0.7, make_frame("app", "t2")).unwrap();
        assert_eq!(flush2.pre_event_frames.len(), 2);
    }

    #[test]
    fn evicted_count_increments_on_full_buffer() {
        let rb = CaptureRingBuffer::new(3, 2, 0.5);
        // Push 5 frames into capacity-3 buffer → 2 evictions
        for i in 0..5 {
            rb.push(make_frame("app", &format!("f-{i}")));
        }
        assert_eq!(rb.evicted_count(), 2);
        assert_eq!(rb.len(), 3);
    }

    #[test]
    fn take_evicted_count_resets() {
        let rb = CaptureRingBuffer::new(2, 1, 0.5);
        // Push 4 frames → 2 evictions
        for i in 0..4 {
            rb.push(make_frame("app", &format!("f-{i}")));
        }
        assert_eq!(rb.take_evicted_count(), 2);
        // Counter should be reset to 0 after take
        assert_eq!(rb.evicted_count(), 0);
        assert_eq!(rb.take_evicted_count(), 0);
    }

    #[test]
    fn no_evictions_under_capacity() {
        let rb = CaptureRingBuffer::new(10, 2, 0.5);
        for i in 0..10 {
            rb.push(make_frame("app", &format!("f-{i}")));
        }
        assert_eq!(rb.evicted_count(), 0);
        assert_eq!(rb.len(), 10);
    }
}
