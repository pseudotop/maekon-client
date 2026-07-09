use chrono::Utc;
use maekon_core::config::{AppConfig, PrivacyConfig};
use maekon_core::models::context::{UserContext, WindowBounds};
use maekon_core::models::focused_element::{AccessibilityElement, FocusedElementInfo};
use maekon_core::ports::vision::FrameProcessor;
use maekon_vision::ring_buffer::{CaptureRingBuffer, RingFrame};
use std::time::{Duration, Instant};

/// Capture-time exclusion decision for one monitor tick (#7909, T1.1).
///
/// The UI promises excluded/sensitive apps are excluded *from capture*, not
/// merely from egress, so the monitor loop gates every content-capture surface
/// of a tick (AX text extraction, ring thumbnail, trigger capture, post-event
/// forced capture, detection re-analysis) on this flag. Metadata context/window
/// events still flow (regime tracking depends on them) — they are PII-sanitized
/// at source and the egress boundary applies the same policy before upload.
///
/// A missing config snapshot falls back to `PrivacyConfig::default()`, whose
/// `auto_exclude_sensitive = true` keeps the sensitive-app auto-detection
/// fail-safe rather than fail-open.
pub(super) fn tick_capture_excluded(
    config: Option<&AppConfig>,
    app_name: &str,
    window_title: &str,
) -> bool {
    match config {
        Some(cfg) => {
            maekon_vision::privacy::should_exclude_by_policy(&cfg.privacy, app_name, window_title)
        }
        None => maekon_vision::privacy::should_exclude_by_policy(
            &PrivacyConfig::default(),
            app_name,
            window_title,
        ),
    }
}

#[derive(Debug, Clone, Default)]
pub(super) struct ActiveWindowSnapshot {
    pub(super) app_name: String,
    pub(super) window_title: String,
    pub(super) window_bounds: Option<WindowBounds>,
    pub(super) app_bundle_id: Option<String>,
}

impl ActiveWindowSnapshot {
    pub(super) fn from_context(ctx: &UserContext) -> Self {
        let Some(window) = ctx.active_window.as_ref() else {
            return Self::default();
        };

        Self {
            app_name: window.app_name.clone(),
            window_title: window.title.clone(),
            window_bounds: window.bounds,
            app_bundle_id: window.app_bundle_id.clone(),
        }
    }
}

#[derive(Default)]
pub(super) struct RingThumbnailCadence {
    last_capture_at: Option<Instant>,
}

impl RingThumbnailCadence {
    pub(super) fn should_capture(&mut self, now: Instant, throttle: Duration) -> bool {
        let throttle = throttle.max(Duration::from_millis(100));
        let should_capture = self
            .last_capture_at
            .map(|last| now.duration_since(last) >= throttle)
            .unwrap_or(true);

        if should_capture {
            self.last_capture_at = Some(now);
        }

        should_capture
    }
}

pub(super) async fn capture_ring_thumbnail_if_due(
    cadence: &mut RingThumbnailCadence,
    processor: &dyn FrameProcessor,
    ring_buffer: &CaptureRingBuffer,
    app_name: &str,
    window_title: &str,
    focused_element: Option<&FocusedElementInfo>,
    throttle: Duration,
) {
    if !cadence.should_capture(Instant::now(), throttle) {
        return;
    }

    if let Ok(thumb_data) = processor.capture_thumbnail().await {
        ring_buffer.push(RingFrame {
            timestamp: Utc::now(),
            thumbnail_data: thumb_data,
            app_name: app_name.to_string(),
            window_title: window_title.to_string(),
            accessibility_elements: focused_element
                .map(|focused| {
                    vec![AccessibilityElement {
                        role: focused.role.clone(),
                        label: focused.label.clone().unwrap_or_default(),
                        bounds: focused.position,
                    }]
                })
                .unwrap_or_default(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maekon_core::models::context::WindowInfo;

    #[test]
    fn active_window_snapshot_defaults_without_window() {
        let ctx = UserContext {
            timestamp: Utc::now(),
            active_window: None,
            processes: vec![],
        };

        let snapshot = ActiveWindowSnapshot::from_context(&ctx);
        assert!(snapshot.app_name.is_empty());
        assert!(snapshot.window_title.is_empty());
        assert!(snapshot.window_bounds.is_none());
        assert!(snapshot.app_bundle_id.is_none());
    }

    #[test]
    fn active_window_snapshot_keeps_window_target_fields() {
        let bounds = WindowBounds {
            x: 10,
            y: 20,
            width: 300,
            height: 200,
        };
        let ctx = UserContext {
            timestamp: Utc::now(),
            active_window: Some(WindowInfo {
                title: "Notes".to_string(),
                app_name: "Notes.app".to_string(),
                app_bundle_id: Some("com.apple.Notes".to_string()),
                pid: 44,
                bounds: Some(bounds),
            }),
            processes: vec![],
        };

        let snapshot = ActiveWindowSnapshot::from_context(&ctx);
        assert_eq!(snapshot.app_name, "Notes.app");
        assert_eq!(snapshot.window_title, "Notes");
        assert!(snapshot.window_bounds.is_some());
        let actual_bounds = snapshot.window_bounds.unwrap();
        assert_eq!(actual_bounds.x, bounds.x);
        assert_eq!(actual_bounds.y, bounds.y);
        assert_eq!(actual_bounds.width, bounds.width);
        assert_eq!(actual_bounds.height, bounds.height);
        assert_eq!(snapshot.app_bundle_id.as_deref(), Some("com.apple.Notes"));
    }

    #[test]
    fn ring_thumbnail_cadence_respects_capture_throttle() {
        let mut cadence = RingThumbnailCadence::default();
        let throttle = Duration::from_secs(5);
        let start = Instant::now();

        assert!(cadence.should_capture(start, throttle));
        assert!(!cadence.should_capture(start + Duration::from_secs(1), throttle));
        assert!(!cadence.should_capture(start + Duration::from_secs(4), throttle));
        assert!(cadence.should_capture(start + Duration::from_secs(5), throttle));
    }

    #[test]
    fn tick_capture_excluded_matches_configured_excluded_app() {
        let mut cfg = AppConfig::default_config();
        cfg.privacy = PrivacyConfig {
            excluded_apps: vec!["Slack".to_string()],
            ..Default::default()
        };

        assert!(tick_capture_excluded(Some(&cfg), "Slack", "General"));
        assert!(!tick_capture_excluded(Some(&cfg), "Chrome", "Google"));
    }

    #[test]
    fn tick_capture_excluded_auto_excludes_sensitive_app_by_default() {
        let cfg = AppConfig::default_config();
        assert!(tick_capture_excluded(Some(&cfg), "1Password", "Unlock"));
    }

    #[test]
    fn tick_capture_excluded_without_config_stays_fail_safe_for_sensitive_apps() {
        // No config snapshot → PrivacyConfig::default() (auto_exclude_sensitive
        // = true) must still auto-exclude sensitive apps.
        assert!(tick_capture_excluded(None, "1Password", "Unlock"));
        assert!(!tick_capture_excluded(None, "Chrome", "Google"));
    }

    #[test]
    fn tick_capture_excluded_respects_auto_exclude_opt_out() {
        let mut cfg = AppConfig::default_config();
        cfg.privacy = PrivacyConfig {
            auto_exclude_sensitive: false,
            ..Default::default()
        };

        assert!(!tick_capture_excluded(Some(&cfg), "1Password", "Unlock"));
    }
}
