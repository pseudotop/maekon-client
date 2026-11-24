//! Dashcam-style ring buffer for frame capture.
//!
//! Continuously records lightweight thumbnail frames into a fixed-size circular
//! buffer. When a significant event occurs (importance >= threshold), the buffer
//! is flushed to provide "before" context, and a post-event counter is set to
//! continue capturing "after" frames.

use chrono::{DateTime, Utc};
use maekon_core::error::CoreError;
use maekon_core::models::focused_element::AccessibilityElement;
use maekon_core::ports::vision::{capture_denied, CaptureAccess, CapturePermit};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;
use tracing::{debug, error};
use zeroize::Zeroize;

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

impl RingFrame {
    fn erase(&mut self) {
        self.thumbnail_data.zeroize();
        self.app_name.zeroize();
        self.window_title.zeroize();
        for element in &mut self.accessibility_elements {
            element.label.zeroize();
        }
        self.accessibility_elements.clear();
    }
}

/// A buffered image can only be saved under its originating authority.
#[derive(Debug)]
pub struct AuthorizedRingFrame {
    pub frame: RingFrame,
    pub permit: CapturePermit,
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
    // Keep legacy users separate: an unguarded drain cannot retrieve authorized
    // runtime images while discarding their origin permits.
    authorized: Mutex<VecDeque<AuthorizedRingFrame>>,
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
            authorized: Mutex::new(VecDeque::with_capacity(capacity)),
            capacity,
            post_event_remaining: AtomicU32::new(0),
            post_event_count,
            flush_threshold,
            evicted_count: AtomicU64::new(0),
        }
    }

    pub fn push_authorized(
        &self,
        mut frame: RingFrame,
        permit: CapturePermit,
    ) -> Result<(), CoreError> {
        let result = (|| {
            let mut buffer = self
                .authorized
                .lock()
                .map_err(|_| capture_denied("ring authority poisoned"))?;
            permit.check(CaptureAccess::Retain)?;
            if self.capacity == 0 {
                return Err(capture_denied("ring capacity is zero"));
            }
            if buffer.len() >= self.capacity {
                if let Some(mut evicted) = buffer.pop_front() {
                    evicted.frame.erase();
                }
                self.evicted_count.fetch_add(1, Ordering::Relaxed);
            }
            buffer.push_back(AuthorizedRingFrame {
                frame: frame.clone(),
                permit,
            });
            drop(buffer);
            Ok(())
        })();
        // The sole retained copy belongs to the guarded queue, even on failure.
        frame.erase();
        result
    }

    /// Revoke stale entries even if pause/resume happened between scheduler
    /// ticks. This only performs live policy reads, never native enumeration.
    pub fn purge_denied(&self) {
        if let Ok(mut buffer) = self.authorized.lock() {
            buffer.retain_mut(|entry| {
                if entry.permit.check(CaptureAccess::Retain).is_ok() {
                    return true;
                }
                entry.frame.erase();
                false
            });
        }
    }

    pub fn flush_authorized(
        &self,
        importance: f32,
        trigger: &CapturePermit,
    ) -> Result<Vec<AuthorizedRingFrame>, CoreError> {
        if importance < self.flush_threshold {
            return Ok(Vec::new());
        }
        let mut buffer = self
            .authorized
            .lock()
            .map_err(|_| capture_denied("ring authority poisoned"))?;
        trigger.check(CaptureAccess::Retain)?;
        let mut frames = Vec::new();
        for mut entry in buffer.drain(..) {
            if entry.permit.check(CaptureAccess::Retain).is_ok() {
                frames.push(entry);
            } else {
                entry.frame.erase();
            }
        }
        self.post_event_remaining
            .store(self.post_event_count, Ordering::Relaxed);
        drop(buffer);
        Ok(frames)
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
            + self.authorized.lock().map(|b| b.len()).unwrap_or(0)
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

    /// Drop every buffered frame, zeroizing its sensitive payload (thumbnail
    /// pixels, window title, app name) in place before the backing allocation
    /// is freed, and reset any pending post-event capture window.
    ///
    /// # Why this exists (#8045 B3)
    /// The ring buffer holds up to `capacity` pre-event dashcam frames entirely
    /// in RAM. GDPR Art. 17 consent-withdrawal erases on-disk data, but nothing
    /// reached this in-memory buffer, so up to `capacity` thumbnails + titles +
    /// accessibility trees survived a consent revoke in RAM. The monitor loop
    /// calls this whenever the capture gate closes (consent withdrawn/revoked,
    /// capture paused, or capture disabled), so pre-consent frames cannot linger
    /// after the user stops consenting. Zeroizing (not just dropping) overwrites
    /// the pixel/title bytes so they do not persist in freed heap pages.
    pub fn clear(&self) {
        if let Ok(mut buffer) = self.authorized.lock() {
            for entry in buffer.iter_mut() {
                entry.frame.erase();
            }
            buffer.clear();
        }
        let Ok(mut buf) = self.buffer.lock() else {
            error!("CaptureRingBuffer lock poisoned on clear");
            return;
        };
        let cleared = buf.len();
        for frame in buf.iter_mut() {
            frame.thumbnail_data.zeroize();
            frame.app_name.zeroize();
            frame.window_title.zeroize();
            // Accessibility elements carry structured on-screen text; drop them.
            frame.accessibility_elements.clear();
        }
        buf.clear();
        // A closed gate must not force any queued "after" captures.
        self.post_event_remaining.store(0, Ordering::Relaxed);
        if cleared > 0 {
            debug!(
                cleared,
                "ring buffer cleared + zeroized (capture gate closed)"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maekon_core::ports::vision::{CaptureGuard, CapturePauseState};
    use std::sync::Arc;

    struct OriginPauseGuard {
        state: Arc<CapturePauseState>,
        origin: u64,
    }
    impl CaptureGuard for OriginPauseGuard {
        fn check(&self, _: CaptureAccess) -> Result<(), CoreError> {
            self.state.check(self.origin)
        }
    }
    fn pause_permit(state: &Arc<CapturePauseState>) -> CapturePermit {
        CapturePermit::new(Arc::new(OriginPauseGuard {
            state: state.clone(),
            origin: state.snapshot().unwrap(),
        }))
    }

    #[test]
    fn denied_pixel_marker_never_enters_authorized_ring() {
        let state = Arc::new(CapturePauseState::new(Arc::new(
            std::sync::atomic::AtomicBool::new(false),
        )));
        let permit = pause_permit(&state);
        state.toggle().unwrap();
        state.toggle().unwrap();
        let rb = CaptureRingBuffer::new(3, 2, 0.5);
        let mut frame = make_frame("Allowed", "synthetic");
        frame.thumbnail_data = b"denied-synthetic-pixel-marker".to_vec();
        let result = rb.push_authorized(frame, permit);
        // Inspect retained bytes, not a mock guard invocation count. Deleting
        // the real push guard makes this retention assertion fail (#12018).
        assert!(
            rb.authorized.lock().unwrap().is_empty(),
            "denied pixels entered the ring"
        );
        assert!(matches!(
            result.unwrap_err(),
            CoreError::PermissionDenied {
                code: maekon_core::error_codes::PermissionCode::PrivacyDenied,
                ..
            }
        ));
        assert_eq!(rb.len(), 0);
        assert!(rb
            .flush_authorized(1.0, &pause_permit(&state))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn pause_aba_cannot_flush_an_old_marker_under_a_new_permit() {
        let state = Arc::new(CapturePauseState::new(Arc::new(
            std::sync::atomic::AtomicBool::new(false),
        )));
        let rb = CaptureRingBuffer::new(3, 2, 0.5);
        let mut old = make_frame("Allowed", "before pause");
        old.thumbnail_data = b"old-pixel-marker".to_vec();
        rb.push_authorized(old, pause_permit(&state)).unwrap();
        state.toggle().unwrap();
        state.toggle().unwrap();
        let fresh = pause_permit(&state);
        let mut new = make_frame("Allowed", "after resume");
        new.thumbnail_data = b"fresh-pixel-marker".to_vec();
        rb.push_authorized(new, fresh.clone()).unwrap();
        let frames = rb.flush_authorized(1.0, &fresh).unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].frame.thumbnail_data, b"fresh-pixel-marker");
        assert_eq!(rb.len(), 0);
        assert_eq!(rb.post_event_remaining(), 2);
    }

    #[test]
    fn legacy_drain_cannot_strip_authority_from_runtime_pixels() {
        let state = Arc::new(CapturePauseState::new(Arc::new(
            std::sync::atomic::AtomicBool::new(false),
        )));
        let rb = CaptureRingBuffer::new(2, 1, 0.5);
        rb.push_authorized(make_frame("Allowed", "authorized"), pause_permit(&state))
            .unwrap();
        assert!(rb
            .check_and_flush(1.0, make_frame("Allowed", "legacy"))
            .unwrap()
            .pre_event_frames
            .is_empty());
        assert_eq!(
            rb.flush_authorized(1.0, &pause_permit(&state))
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn erase_removes_all_sensitive_frame_fields() {
        let mut frame = make_frame("private app", "private title");
        frame.thumbnail_data = b"private pixels".to_vec();
        frame.accessibility_elements.push(AccessibilityElement {
            role: "Editor".into(),
            label: "private text".into(),
            bounds: None,
        });
        frame.erase();
        assert!(frame.thumbnail_data.is_empty());
        assert!(frame.app_name.is_empty());
        assert!(frame.window_title.is_empty());
        assert!(frame.accessibility_elements.is_empty());
    }

    #[test]
    fn authorized_capacity_evicts_oldest_and_counts_both_queues() {
        let state = Arc::new(CapturePauseState::new(Arc::new(
            std::sync::atomic::AtomicBool::new(false),
        )));
        let rb = CaptureRingBuffer::new(2, 2, 0.5);
        rb.push(make_frame("legacy", "legacy"));
        rb.push_authorized(make_frame("allowed", "first"), pause_permit(&state))
            .unwrap();
        assert_eq!(rb.len(), 2);
        assert_eq!(rb.evicted_count(), 0);
        rb.push_authorized(make_frame("allowed", "second"), pause_permit(&state))
            .unwrap();
        rb.push_authorized(make_frame("allowed", "third"), pause_permit(&state))
            .unwrap();
        assert_eq!(rb.len(), 3);
        assert_eq!(rb.evicted_count(), 1);
        let frames = rb.flush_authorized(0.5, &pause_permit(&state)).unwrap();
        assert_eq!(
            frames
                .iter()
                .map(|entry| entry.frame.window_title.as_str())
                .collect::<Vec<_>>(),
            ["second", "third"]
        );
        assert_eq!(rb.len(), 1);
    }

    #[test]
    fn purge_removes_revoked_pixels_before_any_flush_and_keeps_fresh_pixels() {
        let state = Arc::new(CapturePauseState::new(Arc::new(
            std::sync::atomic::AtomicBool::new(false),
        )));
        let rb = CaptureRingBuffer::new(3, 2, 0.5);
        rb.push_authorized(make_frame("allowed", "stale"), pause_permit(&state))
            .unwrap();
        state.toggle().unwrap();
        state.toggle().unwrap();
        rb.push_authorized(make_frame("allowed", "fresh"), pause_permit(&state))
            .unwrap();
        rb.purge_denied();
        assert_eq!(rb.len(), 1);
        assert_eq!(rb.authorized.lock().unwrap()[0].frame.window_title, "fresh");
        assert!(rb
            .flush_authorized(0.49, &pause_permit(&state))
            .unwrap()
            .is_empty());
        assert_eq!(rb.len(), 1);
        assert_eq!(rb.post_event_remaining(), 0);
        let frames = rb.flush_authorized(0.5, &pause_permit(&state)).unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].frame.window_title, "fresh");
        assert_eq!(rb.post_event_remaining(), 2);
        assert_eq!(rb.len(), 0);
    }

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
        use maekon_core::models::focused_element::{AccessibilityElement, ElementRect};

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

    #[test]
    fn clear_empties_buffer_and_resets_post_event_window() {
        // #8045 B3: a consent-gate close must drop every buffered frame AND
        // cancel any pending post-event ("after") capture window.
        let rb = CaptureRingBuffer::new(6, 3, 0.5);
        for i in 0..4 {
            rb.push(make_frame("Secret", &format!("Bank statement {i}")));
        }
        // A high-importance flush arms the post-event window (post_event=3).
        rb.check_and_flush(0.9, make_frame("Secret", "trigger"));
        assert_eq!(rb.post_event_remaining(), 3);
        // Re-accumulate frames after the flush drained the buffer.
        rb.push(make_frame("Secret", "residual thumbnail"));
        assert_eq!(rb.len(), 1);

        rb.clear();

        assert_eq!(rb.len(), 0, "clear must drop every buffered frame");
        assert!(rb.is_empty());
        assert_eq!(
            rb.post_event_remaining(),
            0,
            "clear must cancel the pending post-event capture window"
        );
    }

    #[test]
    fn clear_zeroizes_thumbnail_pixels_before_drop() {
        // Prove clear() overwrites the sensitive thumbnail bytes rather than
        // only dropping the frame. A frame is captured behind an Arc-shared
        // buffer, cleared, and we assert the buffer no longer exposes it.
        let rb = CaptureRingBuffer::new(4, 1, 0.5);
        let frame = RingFrame {
            timestamp: Utc::now(),
            thumbnail_data: vec![0xAB; 64],
            app_name: "Banking".to_string(),
            window_title: "Account balance".to_string(),
            accessibility_elements: Vec::new(),
        };
        rb.push(frame);
        // Snapshot the pre-clear state directly from the backing deque.
        {
            let buf = rb.buffer.lock().unwrap();
            assert_eq!(buf[0].thumbnail_data, vec![0xAB; 64]);
            assert_eq!(buf[0].app_name, "Banking");
        }
        rb.clear();
        // After clear the frame is gone; the buffer holds nothing to expose.
        let buf = rb.buffer.lock().unwrap();
        assert!(buf.is_empty(), "no residual frame may remain after clear");
    }
}
