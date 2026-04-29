use chrono::{DateTime, Utc};
use oneshim_core::models::app_registry::{KeyCategory, KeystrokeProfile};
use oneshim_core::models::event::{InputActivityEvent, KeyboardActivity, MouseActivity};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicI32, AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;
use tracing::error;

pub struct InputActivityCollector {
    period_start: Mutex<DateTime<Utc>>,
    current_app: Mutex<String>,

    click_count: AtomicU32,
    scroll_count: AtomicU32,
    double_click_count: AtomicU32,
    right_click_count: AtomicU32,
    // Last click position — atomic i32 pair (x, y). Updated on record_click_at().
    // Defaults to i32::MIN to distinguish "never set" from (0, 0).
    last_click_x: AtomicI32,
    last_click_y: AtomicI32,
    move_distance: AtomicU64, // f64 save
    total_keystrokes: AtomicU32,
    typing_bursts: AtomicU32,
    shortcut_count: AtomicU32,
    correction_count: AtomicU32,

    // Key-category counters for text-heavy app intelligence (Phase 1).
    // Populated by record_categorized_keystroke(). Remain at zero until
    // Phase 1.5 wires platform key event hooks.
    enter_count: AtomicU32,
    tab_count: AtomicU32,
    arrow_count: AtomicU32,
    backspace_count: AtomicU32,
    special_count: AtomicU32,

    /// Small ring buffer of recent shortcut key strings (e.g., "Cmd+S").
    /// Capacity: 16. Protected by Mutex (low contention — written on shortcut,
    /// drained on snapshot).
    recent_shortcuts: Mutex<VecDeque<String>>,

    last_activity_ms: AtomicU64,
    burst_threshold_ms: u64,
}

impl InputActivityCollector {
    pub fn new() -> Self {
        Self {
            period_start: Mutex::new(Utc::now()),
            current_app: Mutex::new(String::new()),
            click_count: AtomicU32::new(0),
            scroll_count: AtomicU32::new(0),
            double_click_count: AtomicU32::new(0),
            right_click_count: AtomicU32::new(0),
            last_click_x: AtomicI32::new(i32::MIN),
            last_click_y: AtomicI32::new(i32::MIN),
            move_distance: AtomicU64::new(0),
            total_keystrokes: AtomicU32::new(0),
            typing_bursts: AtomicU32::new(0),
            shortcut_count: AtomicU32::new(0),
            correction_count: AtomicU32::new(0),
            enter_count: AtomicU32::new(0),
            tab_count: AtomicU32::new(0),
            arrow_count: AtomicU32::new(0),
            backspace_count: AtomicU32::new(0),
            special_count: AtomicU32::new(0),
            recent_shortcuts: Mutex::new(VecDeque::with_capacity(16)),
            last_activity_ms: AtomicU64::new(0),
            burst_threshold_ms: 2000, // 2 s
        }
    }

    pub fn set_current_app(&self, app_name: &str) {
        if let Ok(mut app) = self.current_app.lock() {
            *app = app_name.to_string();
        }
    }

    pub fn record_click(&self) {
        self.click_count.fetch_add(1, Ordering::Relaxed);
        self.record_activity();
    }

    pub fn record_double_click(&self) {
        self.double_click_count.fetch_add(1, Ordering::Relaxed);
        self.record_activity();
    }

    pub fn record_right_click(&self) {
        self.right_click_count.fetch_add(1, Ordering::Relaxed);
        self.record_activity();
    }

    /// Record a left click at the given screen coordinates.
    /// Position recording is opt-in — callers that lack consent simply call
    /// `record_click()` (which does not update position).
    pub fn record_click_at(&self, x: i32, y: i32) {
        self.click_count.fetch_add(1, Ordering::Relaxed);
        self.last_click_x.store(x, Ordering::Relaxed);
        self.last_click_y.store(y, Ordering::Relaxed);
        self.record_activity();
    }

    pub fn record_scroll(&self) {
        self.scroll_count.fetch_add(1, Ordering::Relaxed);
        self.record_activity();
    }

    pub fn record_mouse_move(&self, distance: f64) {
        let bits = self.move_distance.load(Ordering::Relaxed);
        let current = f64::from_bits(bits);
        let new_bits = (current + distance).to_bits();
        self.move_distance.store(new_bits, Ordering::Relaxed);
    }

    pub fn record_keystroke(&self, is_shortcut: bool, is_correction: bool) {
        self.total_keystrokes.fetch_add(1, Ordering::Relaxed);

        if is_shortcut {
            self.shortcut_count.fetch_add(1, Ordering::Relaxed);
        }
        if is_correction {
            self.correction_count.fetch_add(1, Ordering::Relaxed);
        }

        self.record_activity();
    }

    /// Record a keyboard shortcut with its human-readable name (e.g., "Cmd+S").
    /// Also increments shortcut_count and total_keystrokes.
    pub fn record_shortcut_name(&self, name: &str) {
        self.total_keystrokes.fetch_add(1, Ordering::Relaxed);
        self.shortcut_count.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut buf) = self.recent_shortcuts.lock() {
            if buf.len() >= 16 {
                buf.pop_front();
            }
            buf.push_back(name.to_string());
        }
        self.record_activity();
    }

    /// Record a keystroke with key category classification.
    ///
    /// The caller (platform input hook) classifies each key into one of:
    /// Enter, Tab, Arrow, Backspace, Special, Regular.
    /// Regular keys increment only total_keystrokes.
    /// Category keys increment both their counter AND total_keystrokes.
    pub fn record_categorized_keystroke(
        &self,
        category: KeyCategory,
        is_shortcut: bool,
        is_correction: bool,
    ) {
        self.total_keystrokes.fetch_add(1, Ordering::Relaxed);

        match category {
            KeyCategory::Enter => {
                self.enter_count.fetch_add(1, Ordering::Relaxed);
            }
            KeyCategory::Tab => {
                self.tab_count.fetch_add(1, Ordering::Relaxed);
            }
            KeyCategory::Arrow => {
                self.arrow_count.fetch_add(1, Ordering::Relaxed);
            }
            KeyCategory::Backspace => {
                self.backspace_count.fetch_add(1, Ordering::Relaxed);
                self.correction_count.fetch_add(1, Ordering::Relaxed);
            }
            KeyCategory::Special => {
                self.special_count.fetch_add(1, Ordering::Relaxed);
            }
            KeyCategory::Regular => { /* only total_keystrokes */ }
        }

        // Handle non-Backspace corrections (e.g., Ctrl+Z undo)
        if is_correction && !matches!(category, KeyCategory::Backspace) {
            self.correction_count.fetch_add(1, Ordering::Relaxed);
        }

        if is_shortcut {
            self.shortcut_count.fetch_add(1, Ordering::Relaxed);
        }

        self.record_activity();
    }

    /// Drain and return all recent shortcut names since last call.
    pub fn take_recent_shortcuts(&self) -> Vec<String> {
        self.recent_shortcuts
            .lock()
            .map(|mut buf| buf.drain(..).collect())
            .unwrap_or_default()
    }

    fn record_activity(&self) {
        let now_ms = Utc::now().timestamp_millis() as u64;
        let last_ms = self.last_activity_ms.swap(now_ms, Ordering::Relaxed);

        if last_ms > 0 && now_ms.saturating_sub(last_ms) > self.burst_threshold_ms {
            self.typing_bursts.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Returns a normalized activity score (0.0–1.0) without resetting counters.
    /// Used by the capture trigger to boost importance when input activity is high.
    pub fn peek_activity_level(&self) -> f32 {
        let clicks = self.click_count.load(Ordering::Relaxed);
        let keys = self.total_keystrokes.load(Ordering::Relaxed);
        let scrolls = self.scroll_count.load(Ordering::Relaxed);
        // 10+ clicks or 50+ keystrokes or 20+ scrolls in a period = max activity
        let score = clicks as f32 / 10.0 + keys as f32 / 50.0 + scrolls as f32 / 20.0;
        score.min(1.0)
    }

    pub fn estimate_from_idle_change(&self, prev_idle_secs: u64, curr_idle_secs: u64) {
        if curr_idle_secs < prev_idle_secs {
            let estimated_keystrokes = (prev_idle_secs - curr_idle_secs).min(10) as u32;
            self.total_keystrokes
                .fetch_add(estimated_keystrokes, Ordering::Relaxed);

            self.record_activity();
        }
    }

    pub fn take_snapshot(&self) -> InputActivityEvent {
        let now = Utc::now();

        let period_secs = {
            match self.period_start.lock() {
                Ok(mut start) => {
                    let duration = (now - *start).num_seconds().max(1) as u32;
                    *start = now;
                    duration
                }
                Err(e) => {
                    error!("InputActivityCollector period_start lock poisoned: {e}");
                    1 // safe fallback: treat as a 1-second period
                }
            }
        };

        let app_name = self
            .current_app
            .lock()
            .map(|a| a.clone())
            .unwrap_or_default();

        let clicks = self.click_count.swap(0, Ordering::Relaxed);
        let scrolls = self.scroll_count.swap(0, Ordering::Relaxed);
        let double_clicks = self.double_click_count.swap(0, Ordering::Relaxed);
        let right_clicks = self.right_click_count.swap(0, Ordering::Relaxed);
        let move_bits = self.move_distance.swap(0, Ordering::Relaxed);
        let move_dist = f64::from_bits(move_bits);

        let keystrokes = self.total_keystrokes.swap(0, Ordering::Relaxed);
        let bursts = self.typing_bursts.swap(0, Ordering::Relaxed);
        let shortcuts = self.shortcut_count.swap(0, Ordering::Relaxed);
        let corrections = self.correction_count.swap(0, Ordering::Relaxed);

        let keystrokes_per_min = if period_secs > 0 {
            (keystrokes as f64 / period_secs as f64 * 60.0) as u32
        } else {
            0
        };

        // Key-category counters
        let enters = self.enter_count.swap(0, Ordering::Relaxed);
        let tabs = self.tab_count.swap(0, Ordering::Relaxed);
        let arrows = self.arrow_count.swap(0, Ordering::Relaxed);
        let backspaces = self.backspace_count.swap(0, Ordering::Relaxed);
        let specials = self.special_count.swap(0, Ordering::Relaxed);

        let keystroke_profile = if enters + tabs + arrows + backspaces + specials > 0 {
            let total = keystrokes.max(1) as f32;
            Some(KeystrokeProfile {
                enter_ratio: enters as f32 / total,
                tab_ratio: tabs as f32 / total,
                arrow_ratio: arrows as f32 / total,
                backspace_ratio: backspaces as f32 / total,
                special_ratio: specials as f32 / total,
                total_keystrokes: keystrokes,
            })
        } else {
            None
        };

        InputActivityEvent {
            timestamp: now,
            period_secs,
            mouse: MouseActivity {
                click_count: clicks,
                move_distance: move_dist,
                scroll_count: scrolls,
                last_position: {
                    let lx = self.last_click_x.swap(i32::MIN, Ordering::Relaxed);
                    let ly = self.last_click_y.swap(i32::MIN, Ordering::Relaxed);
                    if lx != i32::MIN && ly != i32::MIN {
                        Some((lx as f32, ly as f32))
                    } else {
                        None
                    }
                },
                double_click_count: double_clicks,
                right_click_count: right_clicks,
            },
            keyboard: KeyboardActivity {
                keystrokes_per_min,
                total_keystrokes: keystrokes,
                typing_bursts: bursts,
                shortcut_count: shortcuts,
                correction_count: corrections,
            },
            app_name,
            keystroke_profile,
        }
    }
}

impl Default for InputActivityCollector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_collector_has_zero_counts() {
        let collector = InputActivityCollector::new();
        let snapshot = collector.take_snapshot();

        assert_eq!(snapshot.mouse.click_count, 0);
        assert_eq!(snapshot.keyboard.total_keystrokes, 0);
    }

    #[test]
    fn records_clicks() {
        let collector = InputActivityCollector::new();
        collector.record_click();
        collector.record_click();
        collector.record_double_click();

        let snapshot = collector.take_snapshot();
        assert_eq!(snapshot.mouse.click_count, 2);
        assert_eq!(snapshot.mouse.double_click_count, 1);
    }

    #[test]
    fn records_keystrokes() {
        let collector = InputActivityCollector::new();
        collector.record_keystroke(false, false);
        collector.record_keystroke(true, false); // shortcut
        collector.record_keystroke(false, true); // correction

        let snapshot = collector.take_snapshot();
        assert_eq!(snapshot.keyboard.total_keystrokes, 3);
        assert_eq!(snapshot.keyboard.shortcut_count, 1);
        assert_eq!(snapshot.keyboard.correction_count, 1);
    }

    #[test]
    fn snapshot_resets_counters() {
        let collector = InputActivityCollector::new();
        collector.record_click();
        collector.record_keystroke(false, false);

        let _ = collector.take_snapshot();
        let second = collector.take_snapshot();

        assert_eq!(second.mouse.click_count, 0);
        assert_eq!(second.keyboard.total_keystrokes, 0);
    }

    #[test]
    fn peek_activity_level_no_activity() {
        let collector = InputActivityCollector::new();
        assert!((collector.peek_activity_level() - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn peek_activity_level_moderate() {
        let collector = InputActivityCollector::new();
        for _ in 0..5 {
            collector.record_click();
        }
        for _ in 0..25 {
            collector.record_keystroke(false, false);
        }
        // 5/10 + 25/50 = 0.5 + 0.5 = 1.0 (capped)
        let level = collector.peek_activity_level();
        assert!((level - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn peek_does_not_reset_counters() {
        let collector = InputActivityCollector::new();
        collector.record_click();
        collector.record_click();

        let level1 = collector.peek_activity_level();
        let level2 = collector.peek_activity_level();
        assert!((level1 - level2).abs() < f32::EPSILON);
        assert!(level1 > 0.0);
    }

    #[test]
    fn estimates_from_idle_change() {
        let collector = InputActivityCollector::new();
        collector.estimate_from_idle_change(10, 0);

        let snapshot = collector.take_snapshot();
        assert!(snapshot.keyboard.total_keystrokes > 0);
    }

    #[test]
    fn record_click_at_sets_position() {
        let collector = InputActivityCollector::new();
        collector.record_click_at(150, 300);

        let snapshot = collector.take_snapshot();
        assert_eq!(snapshot.mouse.click_count, 1);
        assert_eq!(snapshot.mouse.last_position, Some((150.0, 300.0)));
    }

    #[test]
    fn position_resets_after_snapshot() {
        let collector = InputActivityCollector::new();
        collector.record_click_at(100, 200);
        let _ = collector.take_snapshot();

        let second = collector.take_snapshot();
        assert_eq!(second.mouse.last_position, None);
    }

    #[test]
    fn records_shortcut_names() {
        let collector = InputActivityCollector::new();
        collector.record_shortcut_name("Cmd+S");
        collector.record_shortcut_name("Cmd+Z");

        let shortcuts = collector.take_recent_shortcuts();
        assert_eq!(shortcuts, vec!["Cmd+S", "Cmd+Z"]);

        let snapshot = collector.take_snapshot();
        assert_eq!(snapshot.keyboard.shortcut_count, 2);
        assert_eq!(snapshot.keyboard.total_keystrokes, 2);
    }

    #[test]
    fn shortcut_ring_buffer_caps_at_16() {
        let collector = InputActivityCollector::new();
        for i in 0..20 {
            collector.record_shortcut_name(&format!("Key+{i}"));
        }

        let shortcuts = collector.take_recent_shortcuts();
        assert_eq!(shortcuts.len(), 16);
        // Oldest 4 evicted, first remaining is "Key+4"
        assert_eq!(shortcuts[0], "Key+4");
    }

    #[test]
    fn record_categorized_enter() {
        let collector = InputActivityCollector::new();
        collector.record_categorized_keystroke(KeyCategory::Enter, false, false);
        collector.record_categorized_keystroke(KeyCategory::Enter, false, false);
        collector.record_categorized_keystroke(KeyCategory::Regular, false, false);

        let snapshot = collector.take_snapshot();
        assert_eq!(snapshot.keyboard.total_keystrokes, 3);
        let profile = snapshot.keystroke_profile.unwrap();
        assert!((profile.enter_ratio - 2.0 / 3.0).abs() < 0.01);
        assert!((profile.tab_ratio).abs() < f32::EPSILON);
    }

    #[test]
    fn record_categorized_backspace_also_counts_correction() {
        let collector = InputActivityCollector::new();
        collector.record_categorized_keystroke(KeyCategory::Backspace, false, false);

        let snapshot = collector.take_snapshot();
        assert_eq!(snapshot.keyboard.correction_count, 1);
        assert_eq!(snapshot.keyboard.total_keystrokes, 1);
        let profile = snapshot.keystroke_profile.unwrap();
        assert!((profile.backspace_ratio - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn categorized_counters_reset_on_snapshot() {
        let collector = InputActivityCollector::new();
        collector.record_categorized_keystroke(KeyCategory::Tab, false, false);
        let _ = collector.take_snapshot();

        let second = collector.take_snapshot();
        assert!(second.keystroke_profile.is_none());
    }

    #[test]
    fn no_category_keys_yields_none_profile() {
        let collector = InputActivityCollector::new();
        // Only regular keystrokes via the old API
        collector.record_keystroke(false, false);
        collector.record_keystroke(false, false);

        let snapshot = collector.take_snapshot();
        assert!(snapshot.keystroke_profile.is_none());
    }

    #[test]
    fn mixed_categories_produce_correct_ratios() {
        let collector = InputActivityCollector::new();
        // 10 enters, 5 tabs, 5 arrows, 5 regular = 25 total
        for _ in 0..10 {
            collector.record_categorized_keystroke(KeyCategory::Enter, false, false);
        }
        for _ in 0..5 {
            collector.record_categorized_keystroke(KeyCategory::Tab, false, false);
        }
        for _ in 0..5 {
            collector.record_categorized_keystroke(KeyCategory::Arrow, false, false);
        }
        for _ in 0..5 {
            collector.record_categorized_keystroke(KeyCategory::Regular, false, false);
        }

        let snapshot = collector.take_snapshot();
        let profile = snapshot.keystroke_profile.unwrap();
        assert_eq!(profile.total_keystrokes, 25);
        assert!((profile.enter_ratio - 0.4).abs() < 0.01);
        assert!((profile.tab_ratio - 0.2).abs() < 0.01);
        assert!((profile.arrow_ratio - 0.2).abs() < 0.01);
    }
}
