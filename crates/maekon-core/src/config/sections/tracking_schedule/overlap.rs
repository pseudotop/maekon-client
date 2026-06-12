//! Overlap / window-active logic for `TrackingWindow`.
//!
//! Contains `window_is_active` and the weekday conversion helpers.

use chrono::{DateTime, Datelike, TimeZone};

use crate::config::enums::Weekday;
use crate::config::sections::tracking_schedule::types::{parse_hhmm, TrackingWindow};

// ── window_is_active ────────────────────────────────────────────────

impl TrackingWindow {
    /// Return `true` if `now` falls within this window.
    ///
    /// The parameter is generic over `TimeZone` so callers can pass any
    /// `DateTime<Tz>` — `DateTime<Local>` (production), `DateTime<FixedOffset>`
    /// (tests), `DateTime<chrono_tz::Tz>`, etc.  Only `now.time()` and
    /// `now.weekday()` are used; the timezone itself is not inspected.
    ///
    /// Overnight windows (`end < start`) wrap across midnight and match times
    /// in `[start, 24:00)` on the configured day OR `[00:00, end)` on the
    /// following day. Empty `days_of_week` always returns `false`.
    ///
    /// DST notes:
    /// - Spring-forward: no real instant exists for the skipped hour, so
    ///   no call to this method can land in the skipped interval.
    /// - Fall-back: both absolute instants that share the same wall-clock time
    ///   have identical `now.time()` and `now.weekday()`, so both are treated
    ///   identically — if the window covers that wall-clock time, both match.
    pub fn window_is_active<Tz: TimeZone>(&self, now: DateTime<Tz>) -> bool {
        if self.days_of_week.is_empty() {
            return false;
        }

        // Parse start/end — we validate in TryFrom so these are safe to unwrap.
        // If somehow called on an unchecked instance (test construction), treat
        // parse failure as inactive.
        let start_time = match parse_hhmm(&self.start, "start") {
            Ok(t) => t,
            Err(_) => return false,
        };
        let end_time = match parse_hhmm(&self.end, "end") {
            Ok(t) => t,
            Err(_) => return false,
        };

        let now_time = now.time();
        let now_weekday = chrono_weekday_to_ours(now.weekday());

        if end_time > start_time {
            // ── Non-wrapping (same-day) window: [start, end) ──────────────
            // Active only when `now` is on a configured day AND within [start, end).
            self.days_of_week.contains(&now_weekday)
                && now_time >= start_time
                && now_time < end_time
        } else {
            // ── Overnight (wrapping) window ───────────────────────────────
            // The window opens at `start` on the "start-day" and closes at
            // `end` on the following calendar day.
            //
            // `now` is in the window if either:
            //   (A) now_weekday is a configured start-day AND now_time >= start, OR
            //   (B) now_weekday is the day-after a configured start-day AND now_time < end.
            let is_start_day = self.days_of_week.contains(&now_weekday);
            let is_carry_over_day = self
                .days_of_week
                .iter()
                .any(|&d| weekday_succ(d) == now_weekday);

            (is_start_day && now_time >= start_time) || (is_carry_over_day && now_time < end_time)
        }
    }
}

// ── Weekday conversion helpers ──────────────────────────────────────

/// Convert a `chrono::Weekday` to our config `Weekday`.
fn chrono_weekday_to_ours(w: chrono::Weekday) -> Weekday {
    match w {
        chrono::Weekday::Mon => Weekday::Mon,
        chrono::Weekday::Tue => Weekday::Tue,
        chrono::Weekday::Wed => Weekday::Wed,
        chrono::Weekday::Thu => Weekday::Thu,
        chrono::Weekday::Fri => Weekday::Fri,
        chrono::Weekday::Sat => Weekday::Sat,
        chrono::Weekday::Sun => Weekday::Sun,
    }
}

/// Return the day after `d` (wrapping Sun → Mon).
pub(super) fn weekday_succ(d: Weekday) -> Weekday {
    match d {
        Weekday::Mon => Weekday::Tue,
        Weekday::Tue => Weekday::Wed,
        Weekday::Wed => Weekday::Thu,
        Weekday::Thu => Weekday::Fri,
        Weekday::Fri => Weekday::Sat,
        Weekday::Sat => Weekday::Sun,
        Weekday::Sun => Weekday::Mon,
    }
}
