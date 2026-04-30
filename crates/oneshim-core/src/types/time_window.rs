//! Canonical time window primitive — closed-closed `[start, end]` absolute window.
//!
//! Per spec U4: ONESHIM is event-driven business API (Stripe-style), not
//! continuous time-series. Closed-closed semantic matches existing SQL `BETWEEN`
//! and user-facing date range expectations.
//!
//! Wall-clock recurrence types (`TrackingWindow`, coaching `TimeRange`) are
//! intentionally NOT unified — different domain (recurrence vs absolute window).
//!
//! `TimeWindow::new` is the validation-safe constructor. Direct struct literal
//! construction bypasses bound validation — use only when both bounds are known
//! to satisfy `start <= end`.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::error_codes::TimeWindowCode;

/// Closed-bounded absolute time window. Both `start` and `end` are inclusive.
///
/// Validates `start <= end` at construction. Internally always uses `DateTime<Utc>`.
/// External serialization round-trips via RFC3339 ISO8601 strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeWindow {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TimeWindowError {
    #[error("start ({start}) must be <= end ({end})")]
    InvertedBounds {
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    },
    #[error("failed to parse RFC3339 timestamp: {0}")]
    ParseFailed(String),
}

impl TimeWindowError {
    /// Wire code for ADR-019 observability grouping.
    pub fn code(&self) -> TimeWindowCode {
        match self {
            Self::InvertedBounds { .. } => TimeWindowCode::InvertedBounds,
            Self::ParseFailed(_) => TimeWindowCode::ParseFailed,
        }
    }
}

impl TimeWindow {
    /// Construct a TimeWindow with bound validation.
    ///
    /// # Errors
    /// Returns [`TimeWindowError::InvertedBounds`] if `start > end`.
    pub fn new(start: DateTime<Utc>, end: DateTime<Utc>) -> Result<Self, TimeWindowError> {
        if start > end {
            return Err(TimeWindowError::InvertedBounds { start, end });
        }
        Ok(Self { start, end })
    }

    /// Returns true if `instant` is within `[start, end]` (both inclusive).
    pub fn contains(&self, instant: DateTime<Utc>) -> bool {
        instant >= self.start && instant <= self.end
    }

    /// Returns the duration between start and end (always non-negative).
    pub fn duration(&self) -> Duration {
        self.end - self.start
    }

    /// Returns RFC3339 (start, end) pair for SQL parameter binding.
    /// Compatible with existing `WHERE timestamp >= ?1 AND timestamp <= ?2` patterns.
    pub fn to_sql_pair(&self) -> (String, String) {
        (self.start.to_rfc3339(), self.end.to_rfc3339())
    }

    /// Construct a TimeWindow from RFC3339 string pair.
    ///
    /// # Errors
    /// Returns [`TimeWindowError::ParseFailed`] if either string is not valid RFC3339.
    /// Returns [`TimeWindowError::InvertedBounds`] if parsed `start > end`.
    pub fn from_rfc3339_pair(from: &str, to: &str) -> Result<Self, TimeWindowError> {
        let start = DateTime::parse_from_rfc3339(from)
            .map_err(|e| TimeWindowError::ParseFailed(e.to_string()))?
            .with_timezone(&Utc);
        let end = DateTime::parse_from_rfc3339(to)
            .map_err(|e| TimeWindowError::ParseFailed(e.to_string()))?
            .with_timezone(&Utc);
        Self::new(start, end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn dt(y: i32, m: u32, d: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, 0, 0, 0).unwrap()
    }

    #[test]
    fn new_accepts_valid_bounds() {
        let w = TimeWindow::new(dt(2026, 4, 1), dt(2026, 4, 25)).unwrap();
        assert_eq!(w.start, dt(2026, 4, 1));
        assert_eq!(w.end, dt(2026, 4, 25));
    }

    #[test]
    fn new_accepts_zero_duration_window() {
        // Per spec Q-6 RESOLVED: start == end is valid (single-instant query)
        let same = dt(2026, 4, 25);
        let w = TimeWindow::new(same, same).unwrap();
        assert_eq!(w.duration(), Duration::zero());
    }

    #[test]
    fn new_rejects_inverted_bounds() {
        let result = TimeWindow::new(dt(2026, 4, 25), dt(2026, 4, 1));
        assert!(matches!(
            result,
            Err(TimeWindowError::InvertedBounds { .. })
        ));
    }

    #[test]
    fn contains_includes_both_bounds() {
        let w = TimeWindow::new(dt(2026, 4, 1), dt(2026, 4, 25)).unwrap();
        assert!(w.contains(dt(2026, 4, 1))); // start inclusive
        assert!(w.contains(dt(2026, 4, 15))); // middle
        assert!(w.contains(dt(2026, 4, 25))); // end inclusive
    }

    #[test]
    fn contains_excludes_outside() {
        let w = TimeWindow::new(dt(2026, 4, 1), dt(2026, 4, 25)).unwrap();
        assert!(!w.contains(dt(2026, 3, 31)));
        assert!(!w.contains(dt(2026, 4, 26)));
    }

    #[test]
    fn duration_returns_difference() {
        let w = TimeWindow::new(dt(2026, 4, 1), dt(2026, 4, 25)).unwrap();
        assert_eq!(w.duration(), Duration::days(24));
    }

    #[test]
    fn to_sql_pair_round_trips_via_from_rfc3339_pair() {
        let original = TimeWindow::new(dt(2026, 4, 1), dt(2026, 4, 25)).unwrap();
        let (from, to) = original.to_sql_pair();
        let restored = TimeWindow::from_rfc3339_pair(&from, &to).unwrap();
        assert_eq!(original, restored);
    }

    #[test]
    fn from_rfc3339_pair_accepts_z_suffix() {
        // Per spec §8.1 N2: verify both Z and +00:00 work
        let w =
            TimeWindow::from_rfc3339_pair("2026-04-01T00:00:00Z", "2026-04-25T00:00:00Z").unwrap();
        assert_eq!(w.start, dt(2026, 4, 1));
    }

    #[test]
    fn from_rfc3339_pair_handles_timezone_offset() {
        let w = TimeWindow::from_rfc3339_pair(
            "2026-04-01T09:00:00+09:00", // KST
            "2026-04-25T09:00:00+09:00",
        )
        .unwrap();
        // 09:00 KST = 00:00 UTC
        assert_eq!(w.start, dt(2026, 4, 1));
        assert_eq!(w.end, dt(2026, 4, 25));
    }

    #[test]
    fn from_rfc3339_pair_rejects_invalid_strings() {
        let result = TimeWindow::from_rfc3339_pair("not-a-date", "2026-04-25T00:00:00Z");
        assert!(matches!(result, Err(TimeWindowError::ParseFailed(_))));
    }

    #[test]
    fn serde_roundtrip_json() {
        let w = TimeWindow::new(dt(2026, 4, 1), dt(2026, 4, 25)).unwrap();
        let json = serde_json::to_string(&w).unwrap();
        let parsed: TimeWindow = serde_json::from_str(&json).unwrap();
        assert_eq!(w, parsed);
    }

    #[test]
    fn time_window_error_code_inverted_bounds() {
        let err = TimeWindow::new(dt(2026, 4, 25), dt(2026, 4, 1)).unwrap_err();
        assert_eq!(err.code(), TimeWindowCode::InvertedBounds);
    }

    #[test]
    fn time_window_error_code_parse_failed() {
        let err = TimeWindow::from_rfc3339_pair("invalid", "valid").unwrap_err();
        assert_eq!(err.code(), TimeWindowCode::ParseFailed);
    }
}
