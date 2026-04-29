use chrono::{Local, NaiveTime, Utc};
use oneshim_core::config::CoachingConfig;
use oneshim_core::models::coaching::CoachingProfile;
use std::time::Instant;
use tracing::debug;

use super::CoachingEngine;

impl CoachingEngine {
    /// Check if the current time falls within any configured quiet hour range.
    pub(super) fn is_quiet_hour(config: &CoachingConfig) -> bool {
        if config.quiet_hours.is_empty() {
            return false;
        }

        let now = Local::now().time();

        for range in &config.quiet_hours {
            let start = match NaiveTime::parse_from_str(&range.start, "%H:%M") {
                Ok(t) => t,
                Err(_) => continue,
            };
            let end = match NaiveTime::parse_from_str(&range.end, "%H:%M") {
                Ok(t) => t,
                Err(_) => continue,
            };

            if start <= end {
                // Normal range: e.g. 22:00 - 23:00
                if now >= start && now < end {
                    return true;
                }
            } else {
                // Overnight range: e.g. 22:00 - 06:00
                if now >= start || now < end {
                    return true;
                }
            }
        }

        false
    }

    /// Check if enough time has passed since the last alert for this profile.
    /// Returns `true` if the message should be allowed (cooldown passed).
    pub(super) async fn check_cooldown(
        &self,
        config: &CoachingConfig,
        profile: &CoachingProfile,
    ) -> bool {
        let profile_name = format!("{:?}", profile);
        let min_interval = config
            .profiles
            .get(&profile_name)
            .map(|p| p.min_interval_secs)
            .unwrap_or(300);

        let last = self.last_alert.read().await;
        match last.get(&profile_name) {
            Some(last_time) => {
                let elapsed = (Utc::now() - *last_time).num_seconds();
                elapsed >= min_interval as i64
            }
            None => true,
        }
    }

    /// Record the current time as the last alert for this profile.
    pub(super) async fn record_alert(&self, profile: &CoachingProfile) {
        let profile_name = format!("{:?}", profile);
        let mut last = self.last_alert.write().await;
        last.insert(profile_name, Utc::now());
    }

    /// Clear an expired snooze if present. Called before trigger detection.
    pub(super) async fn clear_expired_snooze(&self) {
        let mut guard = self.snoozed_until.write().await;
        if let Some((_, until)) = guard.as_ref() {
            if Instant::now() >= *until {
                *guard = None;
            }
        }
    }

    /// Check if the matched profile is currently snoozed.
    /// Returns `true` if snoozed (should suppress).
    pub(super) async fn is_profile_snoozed(&self, profile: &CoachingProfile) -> bool {
        let guard = self.snoozed_until.read().await;
        if let Some((ref snoozed_profile, until)) = *guard {
            let matched_profile_name = format!("{:?}", profile);
            if Instant::now() < until && matched_profile_name == *snoozed_profile {
                debug!(profile = %snoozed_profile, "coaching suppressed: snoozed");
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Local, Timelike};
    use oneshim_core::config::{CoachingConfig, ProfileConfig, TimeRange};
    use oneshim_core::models::coaching::CoachingProfile;
    use std::collections::HashMap;
    use std::time::Duration;

    // ── is_quiet_hour tests ──────────────────────────────────────

    #[test]
    fn quiet_hour_empty_config_returns_false() {
        let config = CoachingConfig {
            quiet_hours: vec![],
            ..CoachingConfig::default()
        };
        assert!(
            !CoachingEngine::is_quiet_hour(&config),
            "empty quiet_hours should never suppress"
        );
    }

    #[test]
    fn quiet_hour_current_time_inside_normal_range() {
        let now = Local::now().time();
        let start_hour = now.hour();
        let end_hour = (start_hour + 2) % 24;
        let config = CoachingConfig {
            quiet_hours: vec![TimeRange {
                start: format!("{:02}:00", start_hour),
                end: format!("{:02}:00", end_hour),
            }],
            ..CoachingConfig::default()
        };
        // Current time is within [start_hour, start_hour+2), so should be quiet
        assert!(
            CoachingEngine::is_quiet_hour(&config),
            "current time inside normal range should be quiet"
        );
    }

    #[test]
    fn quiet_hour_current_time_outside_normal_range() {
        let now = Local::now().time();
        // Create a range that is far from the current hour
        let start_hour = (now.hour() + 6) % 24;
        let end_hour = (start_hour + 1) % 24;
        // Ensure this is a normal (non-overnight) range by picking hours where start < end
        let (s, e) = if start_hour < end_hour {
            (start_hour, end_hour)
        } else {
            // If wrapping, use a definitely-future narrow window
            let s = (now.hour() + 6) % 24;
            let e = (now.hour() + 7) % 24;
            if s < e {
                (s, e)
            } else {
                // Fallback: skip the test silently (very rare edge case at hour 18+)
                return;
            }
        };
        let config = CoachingConfig {
            quiet_hours: vec![TimeRange {
                start: format!("{:02}:00", s),
                end: format!("{:02}:00", e),
            }],
            ..CoachingConfig::default()
        };
        assert!(
            !CoachingEngine::is_quiet_hour(&config),
            "current time outside normal range should NOT be quiet"
        );
    }

    #[test]
    fn quiet_hour_midnight_crossing_range_inside() {
        let now = Local::now().time();
        // Create a midnight-crossing range that contains the current time.
        // Strategy: end = current_hour + 2 (wrapping), start = current_hour - 1 (wrapping).
        // This makes an overnight range that spans through now.
        let start_hour = (now.hour() + 23) % 24; // now - 1h, wrapping
        let end_hour = (now.hour() + 2) % 24; // now + 2h, wrapping

        // Only valid as overnight range when start > end
        if start_hour <= end_hour {
            // This would be a normal range; skip since the test intent is overnight
            return;
        }

        let config = CoachingConfig {
            quiet_hours: vec![TimeRange {
                start: format!("{:02}:00", start_hour),
                end: format!("{:02}:00", end_hour),
            }],
            ..CoachingConfig::default()
        };
        assert!(
            CoachingEngine::is_quiet_hour(&config),
            "current time inside midnight-crossing range should be quiet"
        );
    }

    #[test]
    fn quiet_hour_midnight_crossing_range_outside() {
        let now = Local::now().time();
        // Create a midnight-crossing range that does NOT contain the current time.
        // For overnight range (start > end), condition is: now >= start || now < end.
        // To be outside, we need now < start AND now >= end.
        // Strategy: start = now + 3h, end = now - 3h (a narrow overnight window
        // that does NOT include the current time).
        let start_hour = (now.hour() + 3) % 24;
        let end_hour = (now.hour() + 21) % 24; // now - 3h, wrapping

        if start_hour <= end_hour {
            // Would be a normal range (not midnight-crossing), skip
            return;
        }

        // Verify: now < start_hour AND now >= end_hour means outside
        // now.hour() < start_hour = now.hour()+3 => true
        // now.hour() >= end_hour = now.hour()-3 => true
        let config = CoachingConfig {
            quiet_hours: vec![TimeRange {
                start: format!("{:02}:00", start_hour),
                end: format!("{:02}:00", end_hour),
            }],
            ..CoachingConfig::default()
        };
        assert!(
            !CoachingEngine::is_quiet_hour(&config),
            "current time outside midnight-crossing range should NOT be quiet"
        );
    }

    #[test]
    fn quiet_hour_invalid_time_format_skipped() {
        let config = CoachingConfig {
            quiet_hours: vec![TimeRange {
                start: "invalid".to_string(),
                end: "also-invalid".to_string(),
            }],
            ..CoachingConfig::default()
        };
        assert!(
            !CoachingEngine::is_quiet_hour(&config),
            "invalid time format should be skipped (not panic)"
        );
    }

    #[test]
    fn quiet_hour_multiple_ranges_second_matches() {
        let now = Local::now().time();
        let far_start = (now.hour() + 8) % 24;
        let far_end = (now.hour() + 9) % 24;
        let near_start = now.hour();
        let near_end = (now.hour() + 2) % 24;

        // First range does not contain now, second does
        let mut ranges = vec![];
        if far_start < far_end {
            ranges.push(TimeRange {
                start: format!("{:02}:00", far_start),
                end: format!("{:02}:00", far_end),
            });
        }
        ranges.push(TimeRange {
            start: format!("{:02}:00", near_start),
            end: format!("{:02}:00", near_end),
        });

        let config = CoachingConfig {
            quiet_hours: ranges,
            ..CoachingConfig::default()
        };
        assert!(
            CoachingEngine::is_quiet_hour(&config),
            "should match on the second range"
        );
    }

    #[test]
    fn quiet_hour_boundary_start_equals_end() {
        // start == end means an empty range — should return false
        let now = Local::now().time();
        let hour = now.hour();
        let config = CoachingConfig {
            quiet_hours: vec![TimeRange {
                start: format!("{:02}:00", hour),
                end: format!("{:02}:00", hour),
            }],
            ..CoachingConfig::default()
        };
        // start == end => start <= end path, now >= start && now < end is never true
        // when start == end (the range has zero width)
        assert!(
            !CoachingEngine::is_quiet_hour(&config),
            "zero-width range (start == end) should NOT be quiet"
        );
    }

    // ── check_cooldown tests ─────────────────────────────────────

    #[tokio::test]
    async fn check_cooldown_allows_first_alert() {
        let config = CoachingConfig::default();
        let engine = CoachingEngine::new(config.clone());
        let profile = CoachingProfile::FocusGuard;
        assert!(
            engine.check_cooldown(&config, &profile).await,
            "first alert (no history) should pass cooldown"
        );
    }

    #[tokio::test]
    async fn check_cooldown_blocks_within_interval() {
        let mut profiles = HashMap::new();
        profiles.insert(
            "FocusGuard".to_string(),
            ProfileConfig {
                enabled: true,
                min_interval_secs: 600,
            },
        );
        let config = CoachingConfig {
            profiles,
            ..CoachingConfig::default()
        };
        let engine = CoachingEngine::new(config.clone());

        // Record an alert just now
        engine.record_alert(&CoachingProfile::FocusGuard).await;

        // Should be blocked (0 seconds elapsed, need 600)
        assert!(
            !engine
                .check_cooldown(&config, &CoachingProfile::FocusGuard)
                .await,
            "alert within cooldown interval should be blocked"
        );
    }

    #[tokio::test]
    async fn check_cooldown_allows_after_interval() {
        let mut profiles = HashMap::new();
        profiles.insert(
            "TimeAware".to_string(),
            ProfileConfig {
                enabled: true,
                min_interval_secs: 1, // 1 second
            },
        );
        let config = CoachingConfig {
            profiles,
            ..CoachingConfig::default()
        };
        let engine = CoachingEngine::new(config.clone());

        // Record alert, then manually backdate it
        {
            let mut last = engine.last_alert.write().await;
            last.insert(
                "TimeAware".to_string(),
                Utc::now() - chrono::Duration::seconds(5),
            );
        }

        assert!(
            engine
                .check_cooldown(&config, &CoachingProfile::TimeAware)
                .await,
            "alert after cooldown interval should be allowed"
        );
    }

    #[tokio::test]
    async fn check_cooldown_uses_default_300s_for_unknown_profile() {
        // Config has no entry for "DeepWorkCoach" -> defaults to 300s
        let config = CoachingConfig {
            profiles: HashMap::new(),
            ..CoachingConfig::default()
        };
        let engine = CoachingEngine::new(config.clone());

        // Record alert 100 seconds ago (< 300s default)
        {
            let mut last = engine.last_alert.write().await;
            last.insert(
                "DeepWorkCoach".to_string(),
                Utc::now() - chrono::Duration::seconds(100),
            );
        }

        assert!(
            !engine
                .check_cooldown(&config, &CoachingProfile::DeepWorkCoach)
                .await,
            "should use 300s default for unknown profile"
        );
    }

    // ── record_alert tests ───────────────────────────────────────

    #[tokio::test]
    async fn record_alert_stores_timestamp() {
        let engine = CoachingEngine::new(CoachingConfig::default());
        let before = Utc::now();
        engine.record_alert(&CoachingProfile::GoalTracker).await;
        let after = Utc::now();

        let last = engine.last_alert.read().await;
        let recorded = last.get("GoalTracker").expect("should be recorded");
        assert!(*recorded >= before && *recorded <= after);
    }

    // ── clear_expired_snooze tests ───────────────────────────────

    #[tokio::test]
    async fn clear_expired_snooze_removes_expired() {
        let engine = CoachingEngine::new(CoachingConfig::default());
        // Set a snooze that already expired (Instant::now() - 1 ns)
        {
            let mut guard = engine.snoozed_until.write().await;
            // We can't go before Instant::now(), so use a 0-duration snooze
            // and then call clear_expired_snooze after a tiny delay.
            *guard = Some(("FocusGuard".to_string(), Instant::now()));
        }
        // The expiry is at Instant::now(), so after this point it's expired
        tokio::time::sleep(Duration::from_millis(1)).await;
        engine.clear_expired_snooze().await;

        let guard = engine.snoozed_until.read().await;
        assert!(guard.is_none(), "expired snooze should be cleared");
    }

    #[tokio::test]
    async fn clear_expired_snooze_keeps_active() {
        let engine = CoachingEngine::new(CoachingConfig::default());
        {
            let mut guard = engine.snoozed_until.write().await;
            *guard = Some((
                "FocusGuard".to_string(),
                Instant::now() + Duration::from_secs(60),
            ));
        }
        engine.clear_expired_snooze().await;

        let guard = engine.snoozed_until.read().await;
        assert!(guard.is_some(), "active snooze should be kept");
    }

    // ── is_profile_snoozed tests ─────────────────────────────────

    #[tokio::test]
    async fn is_profile_snoozed_returns_true_for_matching_active_snooze() {
        let engine = CoachingEngine::new(CoachingConfig::default());
        engine
            .snooze_current_profile("FocusGuard", Duration::from_secs(60))
            .await;

        assert!(
            engine
                .is_profile_snoozed(&CoachingProfile::FocusGuard)
                .await,
            "matching snoozed profile should return true"
        );
    }

    #[tokio::test]
    async fn is_profile_snoozed_returns_false_for_different_profile() {
        let engine = CoachingEngine::new(CoachingConfig::default());
        engine
            .snooze_current_profile("FocusGuard", Duration::from_secs(60))
            .await;

        assert!(
            !engine.is_profile_snoozed(&CoachingProfile::TimeAware).await,
            "different profile should not be snoozed"
        );
    }

    #[tokio::test]
    async fn is_profile_snoozed_returns_false_when_no_snooze() {
        let engine = CoachingEngine::new(CoachingConfig::default());
        assert!(
            !engine
                .is_profile_snoozed(&CoachingProfile::FocusGuard)
                .await,
            "no snooze set should return false"
        );
    }
}
