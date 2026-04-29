use chrono::Utc;
use oneshim_core::models::context::WindowBounds;
use oneshim_core::models::event::{WindowInfo, WindowLayoutEvent, WindowLayoutEventType};
use std::sync::Mutex;

#[derive(Debug, Clone)]
struct PreviousWindowState {
    app_name: String,
    window_title: String,
    position: (i32, i32),
    size: (u32, u32),
    is_fullscreen: bool,
}

pub struct WindowLayoutTracker {
    prev_state: Mutex<Option<PreviousWindowState>>,
    screen_resolution: Mutex<(u32, u32)>,
    position_threshold: i32,
    size_threshold: u32,
}

impl WindowLayoutTracker {
    pub fn new() -> Self {
        Self {
            prev_state: Mutex::new(None),
            screen_resolution: Mutex::new((1920, 1080)), // default value
            position_threshold: 5,                       // ignore <= 5 px moves
            size_threshold: 10,                          // ignore <= 10 px resizes
        }
    }

    pub fn set_screen_resolution(&self, width: u32, height: u32) {
        if let Ok(mut res) = self.screen_resolution.lock() {
            *res = (width, height);
        }
    }

    fn get_screen_resolution(&self) -> (u32, u32) {
        self.screen_resolution
            .lock()
            .map(|r| *r)
            .unwrap_or((1920, 1080))
    }

    pub fn update(
        &self,
        app_name: &str,
        window_title: &str,
        bounds: Option<WindowBounds>,
    ) -> Option<WindowLayoutEvent> {
        let current = match bounds {
            Some(b) => PreviousWindowState {
                app_name: app_name.to_string(),
                window_title: window_title.to_string(),
                position: (b.x, b.y),
                size: (b.width, b.height),
                is_fullscreen: self.is_fullscreen(&b),
            },
            None => return None,
        };

        let mut prev_guard = self.prev_state.lock().ok()?;
        let event_type = self.detect_change(&prev_guard, &current);

        let _prev = prev_guard.replace(current.clone());
        drop(prev_guard);

        let event_type = event_type?;

        let screen_res = self.get_screen_resolution();
        let screen_ratio = self.calculate_screen_ratio(&current, screen_res);

        Some(WindowLayoutEvent {
            timestamp: Utc::now(),
            event_type,
            window: WindowInfo {
                app_name: current.app_name,
                window_title: current.window_title,
                position: current.position,
                size: current.size,
                screen_ratio,
                is_fullscreen: current.is_fullscreen,
                z_order: 0, // top-most active window
            },
            screen_resolution: screen_res,
            monitor_index: 0, // default monitor
        })
    }

    fn detect_change(
        &self,
        prev: &Option<PreviousWindowState>,
        current: &PreviousWindowState,
    ) -> Option<WindowLayoutEventType> {
        let prev = match prev {
            Some(p) => p,
            None => return Some(WindowLayoutEventType::Focus), // first window
        };

        if prev.app_name != current.app_name || prev.window_title != current.window_title {
            return Some(WindowLayoutEventType::Focus);
        }

        if !prev.is_fullscreen && current.is_fullscreen {
            return Some(WindowLayoutEventType::Maximize);
        }
        if prev.is_fullscreen && !current.is_fullscreen {
            return Some(WindowLayoutEventType::Restore);
        }

        let width_diff = (current.size.0 as i32 - prev.size.0 as i32).unsigned_abs();
        let height_diff = (current.size.1 as i32 - prev.size.1 as i32).unsigned_abs();
        if width_diff > self.size_threshold || height_diff > self.size_threshold {
            return Some(WindowLayoutEventType::Resize);
        }

        let x_diff = (current.position.0 - prev.position.0).abs();
        let y_diff = (current.position.1 - prev.position.1).abs();
        if x_diff > self.position_threshold || y_diff > self.position_threshold {
            return Some(WindowLayoutEventType::Move);
        }

        None
    }

    fn is_fullscreen(&self, bounds: &WindowBounds) -> bool {
        let screen_res = self.get_screen_resolution();
        let width_match = (bounds.width as i32 - screen_res.0 as i32).abs() < 50;
        let height_match = (bounds.height as i32 - screen_res.1 as i32).abs() < 100; // allow menu bar / dock
        let pos_match = bounds.x.abs() < 50 && bounds.y.abs() < 100;

        width_match && height_match && pos_match
    }

    fn calculate_screen_ratio(&self, state: &PreviousWindowState, screen_res: (u32, u32)) -> f32 {
        let window_area = state.size.0 as f64 * state.size.1 as f64;
        let screen_area = screen_res.0 as f64 * screen_res.1 as f64;
        if screen_area > 0.0 {
            (window_area / screen_area) as f32
        } else {
            0.0
        }
    }

    #[allow(dead_code)]
    pub fn detect_minimize(&self, bounds: Option<WindowBounds>) -> bool {
        match bounds {
            Some(b) => b.width < 100 || b.height < 100,
            None => true,
        }
    }
}

impl Default for WindowLayoutTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_bounds(x: i32, y: i32, w: u32, h: u32) -> WindowBounds {
        WindowBounds {
            x,
            y,
            width: w,
            height: h,
        }
    }

    #[test]
    fn first_window_is_focus_event() {
        let tracker = WindowLayoutTracker::new();
        let event = tracker.update("Code", "main.rs", Some(make_bounds(0, 0, 800, 600)));

        assert!(event.is_some());
        assert!(matches!(
            event.unwrap().event_type,
            WindowLayoutEventType::Focus
        ));
    }

    #[test]
    fn app_change_is_focus_event() {
        let tracker = WindowLayoutTracker::new();
        tracker.update("Code", "main.rs", Some(make_bounds(0, 0, 800, 600)));
        let event = tracker.update("Chrome", "Google", Some(make_bounds(0, 0, 800, 600)));

        assert!(event.is_some());
        assert!(matches!(
            event.unwrap().event_type,
            WindowLayoutEventType::Focus
        ));
    }

    #[test]
    fn size_change_is_resize_event() {
        let tracker = WindowLayoutTracker::new();
        tracker.update("Code", "main.rs", Some(make_bounds(0, 0, 800, 600)));
        let event = tracker.update("Code", "main.rs", Some(make_bounds(0, 0, 1000, 800)));

        assert!(event.is_some());
        assert!(matches!(
            event.unwrap().event_type,
            WindowLayoutEventType::Resize
        ));
    }

    #[test]
    fn position_change_is_move_event() {
        let tracker = WindowLayoutTracker::new();
        tracker.update("Code", "main.rs", Some(make_bounds(0, 0, 800, 600)));
        let event = tracker.update("Code", "main.rs", Some(make_bounds(100, 100, 800, 600)));

        assert!(event.is_some());
        assert!(matches!(
            event.unwrap().event_type,
            WindowLayoutEventType::Move
        ));
    }

    #[test]
    fn small_changes_ignored() {
        let tracker = WindowLayoutTracker::new();
        tracker.update("Code", "main.rs", Some(make_bounds(0, 0, 800, 600)));
        let event = tracker.update("Code", "main.rs", Some(make_bounds(3, 3, 800, 600)));

        assert!(event.is_none());
    }

    #[test]
    fn fullscreen_detection() {
        let tracker = WindowLayoutTracker::new();
        tracker.set_screen_resolution(1920, 1080);
        tracker.update("Code", "main.rs", Some(make_bounds(0, 0, 800, 600)));
        let event = tracker.update("Code", "main.rs", Some(make_bounds(0, 0, 1920, 1080)));

        assert!(event.is_some());
        assert!(matches!(
            event.unwrap().event_type,
            WindowLayoutEventType::Maximize
        ));
    }

    #[test]
    fn screen_ratio_calculation() {
        let tracker = WindowLayoutTracker::new();
        tracker.set_screen_resolution(1920, 1080);
        let event = tracker
            .update("Code", "main.rs", Some(make_bounds(0, 0, 960, 540)))
            .unwrap();

        // 960x540 / 1920x1080 = 0.25
        assert!((event.window.screen_ratio - 0.25).abs() < 0.01);
    }
}
