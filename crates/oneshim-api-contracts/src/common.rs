use chrono::{DateTime, Duration, Utc};
use oneshim_core::types::{TimeWindow, TimeWindowError};
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Deserialize)]
pub struct TimeRangeQuery {
    pub from: Option<String>,
    pub to: Option<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    pub min_importance: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct PaginationMeta {
    pub total: u64,
    pub offset: usize,
    pub limit: usize,
    pub has_more: bool,
}

#[derive(Debug, Serialize)]
pub struct PaginatedResponse<T: Serialize> {
    pub data: Vec<T>,
    pub pagination: PaginationMeta,
}

impl TimeRangeQuery {
    pub fn from_datetime(&self) -> DateTime<Utc> {
        self.from
            .as_ref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|| Utc::now() - Duration::hours(24))
    }

    pub fn to_datetime(&self) -> DateTime<Utc> {
        self.to
            .as_ref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(Utc::now)
    }

    pub fn limit_or_default(&self) -> usize {
        self.limit.unwrap_or(100)
    }

    pub fn offset_or_default(&self) -> usize {
        self.offset.unwrap_or(0)
    }

    /// Convert REST query optional bounds into a bounded `TimeWindow`.
    ///
    /// - If `to` is None: defaults to `now()`.
    /// - If `from` is None: defaults to `to - default_lookback`.
    /// - `default_lookback` is domain-specific (e.g. `Duration::hours(24)` to
    ///   preserve `from_datetime()` semantics).
    ///
    /// Per spec U5: this is the boundary where Optional bounds become Required
    /// bounds. Internal code (storage, models) work with `TimeWindow`.
    ///
    /// Per Phase 1 iter-1 C4: takes `&self` (not `self`) so service sites that
    /// pass `&TimeRangeQuery` and continue to use `limit`/`offset`/`min_importance`
    /// fields don't need to clone or restructure.
    ///
    /// # Errors
    /// - [`TimeWindowError::ParseFailed`] if `from` or `to` is not valid RFC3339.
    /// - [`TimeWindowError::InvertedBounds`] if parsed `start > end`.
    pub fn to_time_window(
        &self,
        default_lookback: Duration,
    ) -> Result<TimeWindow, TimeWindowError> {
        let now = Utc::now();
        let end = match self.to.as_deref() {
            Some(s) => DateTime::parse_from_rfc3339(s)
                .map_err(|e| TimeWindowError::ParseFailed(e.to_string()))?
                .with_timezone(&Utc),
            None => now,
        };
        let start = match self.from.as_deref() {
            Some(s) => DateTime::parse_from_rfc3339(s)
                .map_err(|e| TimeWindowError::ParseFailed(e.to_string()))?
                .with_timezone(&Utc),
            None => end - default_lookback,
        };
        TimeWindow::new(start, end)
    }
}

#[cfg(test)]
mod time_window_adapter_tests {
    use super::*;
    use chrono::{Duration, TimeZone};

    fn dt(y: i32, m: u32, d: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, 0, 0, 0).unwrap()
    }

    #[test]
    fn to_time_window_with_both_bounds_provided() {
        let q = TimeRangeQuery {
            from: Some("2026-04-01T00:00:00Z".to_string()),
            to: Some("2026-04-25T00:00:00Z".to_string()),
            ..Default::default()
        };
        let w = q.to_time_window(Duration::days(7)).unwrap();
        assert_eq!(w.start, dt(2026, 4, 1));
        assert_eq!(w.end, dt(2026, 4, 25));
    }

    #[test]
    fn to_time_window_default_to_when_to_missing() {
        let q = TimeRangeQuery {
            from: Some("2026-04-01T00:00:00Z".to_string()),
            to: None,
            ..Default::default()
        };
        let before = Utc::now();
        let w = q.to_time_window(Duration::days(7)).unwrap();
        let after = Utc::now();
        assert!(w.end >= before && w.end <= after);
    }

    #[test]
    fn to_time_window_default_lookback_when_from_missing() {
        let q = TimeRangeQuery {
            from: None,
            to: Some("2026-04-25T00:00:00Z".to_string()),
            ..Default::default()
        };
        let w = q.to_time_window(Duration::days(7)).unwrap();
        // start = to - 7 days = 2026-04-18
        assert_eq!(w.end, dt(2026, 4, 25));
        assert_eq!(w.start, dt(2026, 4, 18));
        assert_eq!(w.duration(), Duration::days(7));
    }

    #[test]
    fn to_time_window_default_both_when_neither_provided() {
        let q = TimeRangeQuery {
            from: None,
            to: None,
            ..Default::default()
        };
        let w = q.to_time_window(Duration::days(7)).unwrap();
        assert_eq!(w.duration(), Duration::days(7));
    }

    #[test]
    fn to_time_window_rejects_invalid_iso8601_from() {
        let q = TimeRangeQuery {
            from: Some("not-a-date".to_string()),
            to: None,
            ..Default::default()
        };
        let result = q.to_time_window(Duration::days(7));
        assert!(matches!(result, Err(TimeWindowError::ParseFailed(_))));
    }

    #[test]
    fn to_time_window_rejects_invalid_iso8601_to() {
        let q = TimeRangeQuery {
            from: None,
            to: Some("also-not-a-date".to_string()),
            ..Default::default()
        };
        let result = q.to_time_window(Duration::days(7));
        assert!(matches!(result, Err(TimeWindowError::ParseFailed(_))));
    }

    #[test]
    fn to_time_window_rejects_inverted_bounds() {
        let q = TimeRangeQuery {
            from: Some("2026-04-25T00:00:00Z".to_string()),
            to: Some("2026-04-01T00:00:00Z".to_string()),
            ..Default::default()
        };
        let result = q.to_time_window(Duration::days(7));
        assert!(matches!(
            result,
            Err(TimeWindowError::InvertedBounds { .. })
        ));
    }

    #[test]
    fn to_time_window_takes_ref_so_caller_keeps_other_fields() {
        // Phase 1 iter-1 C4 verification: &self adapter doesn't consume q
        let q = TimeRangeQuery {
            from: Some("2026-04-01T00:00:00Z".to_string()),
            to: Some("2026-04-25T00:00:00Z".to_string()),
            limit: Some(50),
            ..Default::default()
        };
        let _w = q.to_time_window(Duration::days(7)).unwrap();
        // q still usable after adapter call
        assert_eq!(q.limit, Some(50));
    }
}
