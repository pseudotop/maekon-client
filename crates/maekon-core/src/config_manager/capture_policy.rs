//! Publication identity for the policy consumed by live capture permits.
//! Keep the capture-gate schedule, vision processing and pixel/text privacy
//! inputs together. Notification/transport/UI changes are not image revocation.

use crate::config::AppConfig;

pub(super) fn changed(before: &AppConfig, after: &AppConfig) -> bool {
    let vision = |config: &AppConfig| {
        let v = &config.vision;
        (
            v.capture_enabled,
            v.capture_throttle_ms,
            v.thumbnail_width,
            v.thumbnail_height,
            v.ocr_enabled,
            v.privacy_mode,
        )
    };
    let schedule = |config: &AppConfig| {
        let s = &config.schedule;
        (
            s.active_hours_enabled,
            s.active_start_hour,
            s.active_end_hour,
            s.pause_on_screen_lock,
            s.pause_on_battery_saver,
        )
    };
    let left = &before.privacy;
    let right = &after.privacy;
    let left_tracking = &before.tracking_schedule;
    let right_tracking = &after.tracking_schedule;
    vision(before) != vision(after)
        || before.vision.ocr_languages != after.vision.ocr_languages
        || schedule(before) != schedule(after)
        || before.schedule.active_days != after.schedule.active_days
        || left.excluded_apps != right.excluded_apps
        || left.excluded_app_patterns != right.excluded_app_patterns
        || left.excluded_title_patterns != right.excluded_title_patterns
        || left.auto_exclude_sensitive != right.auto_exclude_sensitive
        || left.pii_filter_level != right.pii_filter_level
        || left_tracking.enabled != right_tracking.enabled
        || left_tracking.timezone != right_tracking.timezone
        // Labels describe a window; they do not change when capture is allowed.
        || !left_tracking.windows.iter().map(|w| (&w.start, &w.end, &w.days_of_week)).eq(
            right_tracking.windows.iter().map(|w| (&w.start, &w.end, &w.days_of_week)),
        )
}
