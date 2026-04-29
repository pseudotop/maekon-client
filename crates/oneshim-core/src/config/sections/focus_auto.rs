use chrono::Timelike;
use serde::{Deserialize, Serialize};

use super::super::enums::Weekday;
use super::coaching::TimeRange;

/// Focus auto-switch configuration — rules for automatic focus mode activation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FocusAutoConfig {
    /// Master toggle for auto-switch rules.
    pub enabled: bool,
    /// Duration in minutes for auto-activated focus sessions (0 = indefinite).
    pub duration_minutes: u32,
    /// App display names that trigger focus mode when in foreground.
    pub trigger_apps: Vec<String>,
    /// Time windows during which focus mode auto-activates.
    pub trigger_schedules: Vec<FocusSchedule>,
    /// Cooldown seconds after deactivation before re-triggering.
    pub cooldown_secs: u64,
}

impl Default for FocusAutoConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            duration_minutes: 25,
            trigger_apps: vec![],
            trigger_schedules: vec![],
            cooldown_secs: 300,
        }
    }
}

/// A scheduled time window for automatic focus activation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FocusSchedule {
    /// Start/end times in "HH:MM" format (reuses coaching TimeRange).
    #[serde(flatten)]
    pub time_range: TimeRange,
    /// Days of the week this schedule applies. Empty = every day.
    #[serde(default)]
    pub days: Vec<Weekday>,
}

impl FocusSchedule {
    /// Check if the schedule matches the current local time and weekday.
    pub fn matches_now(&self, now: &chrono::DateTime<chrono::Local>) -> bool {
        use chrono::Datelike;

        // Parse start/end times
        let Some((sh, sm)) = parse_hhmm(&self.time_range.start) else {
            return false;
        };
        let Some((eh, em)) = parse_hhmm(&self.time_range.end) else {
            return false;
        };

        // Check day filter (empty = every day)
        if !self.days.is_empty() {
            let today = now.weekday().num_days_from_sunday();
            if !self.days.iter().any(|d| d.num_days_from_sunday() == today) {
                return false;
            }
        }

        let current_mins = now.hour() * 60 + now.minute();
        let start_mins = sh * 60 + sm;
        let end_mins = eh * 60 + em;

        if start_mins <= end_mins {
            // Same-day range (e.g., 09:00 - 17:00)
            current_mins >= start_mins && current_mins < end_mins
        } else {
            // Overnight range (e.g., 22:00 - 06:00)
            current_mins >= start_mins || current_mins < end_mins
        }
    }
}

fn parse_hhmm(s: &str) -> Option<(u32, u32)> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 2 {
        return None;
    }
    let h = parts[0].parse::<u32>().ok()?;
    let m = parts[1].parse::<u32>().ok()?;
    if h >= 24 || m >= 60 {
        return None;
    }
    Some((h, m))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn schedule(start: &str, end: &str, days: Vec<Weekday>) -> FocusSchedule {
        FocusSchedule {
            time_range: TimeRange {
                start: start.to_string(),
                end: end.to_string(),
            },
            days,
        }
    }

    #[test]
    fn matches_within_range() {
        let s = schedule("09:00", "17:00", vec![]);
        let now = chrono::Local
            .with_ymd_and_hms(2026, 4, 10, 10, 30, 0)
            .unwrap();
        assert!(s.matches_now(&now));
    }

    #[test]
    fn outside_range() {
        let s = schedule("09:00", "17:00", vec![]);
        let now = chrono::Local
            .with_ymd_and_hms(2026, 4, 10, 18, 0, 0)
            .unwrap();
        assert!(!s.matches_now(&now));
    }

    #[test]
    fn day_filter_matches() {
        // 2026-04-10 is a Friday
        let s = schedule("09:00", "17:00", vec![Weekday::Fri]);
        let now = chrono::Local
            .with_ymd_and_hms(2026, 4, 10, 10, 0, 0)
            .unwrap();
        assert!(s.matches_now(&now));
    }

    #[test]
    fn day_filter_rejects() {
        // 2026-04-10 is a Friday
        let s = schedule("09:00", "17:00", vec![Weekday::Mon]);
        let now = chrono::Local
            .with_ymd_and_hms(2026, 4, 10, 10, 0, 0)
            .unwrap();
        assert!(!s.matches_now(&now));
    }

    #[test]
    fn empty_days_matches_any() {
        let s = schedule("09:00", "17:00", vec![]);
        let now = chrono::Local
            .with_ymd_and_hms(2026, 4, 10, 10, 0, 0)
            .unwrap();
        assert!(s.matches_now(&now));
    }

    #[test]
    fn invalid_time_returns_false() {
        let s = schedule("25:00", "17:00", vec![]);
        let now = chrono::Local
            .with_ymd_and_hms(2026, 4, 10, 10, 0, 0)
            .unwrap();
        assert!(!s.matches_now(&now));
    }

    #[test]
    fn default_config_is_disabled() {
        let c = FocusAutoConfig::default();
        assert!(!c.enabled);
        assert_eq!(c.duration_minutes, 25);
        assert_eq!(c.cooldown_secs, 300);
    }
}
