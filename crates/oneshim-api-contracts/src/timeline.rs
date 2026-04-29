use chrono::{DateTime, Duration, Utc};
use oneshim_core::types::{TimeWindow, TimeWindowError};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
pub struct SessionInfo {
    pub start: String,
    pub end: String,
    pub duration_secs: i64,
    pub total_events: i64,
    pub total_frames: i64,
    pub total_idle_secs: i64,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum TimelineItem {
    Event {
        id: String,
        timestamp: String,
        event_type: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        app_name: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        window_title: Option<String>,
    },
    Frame {
        id: i64,
        timestamp: String,
        app_name: String,
        window_title: String,
        importance: f32,
        image_url: String,
    },
    IdlePeriod {
        start: String,
        end: String,
        duration_secs: i64,
    },
}

#[derive(Debug, Serialize)]
pub struct AppSegment {
    pub app_name: String,
    pub start: String,
    pub end: String,
    pub color: String,
}

#[derive(Debug, Serialize)]
pub struct TimelineResponse {
    pub session: SessionInfo,
    pub items: Vec<TimelineItem>,
    pub segments: Vec<AppSegment>,
}

#[derive(Debug, Deserialize)]
pub struct TimelineQuery {
    pub from: Option<String>,
    pub to: Option<String>,
    pub max_events: Option<usize>,
    pub max_frames: Option<usize>,
}

impl TimelineQuery {
    pub fn from_datetime(&self) -> DateTime<Utc> {
        self.from
            .as_ref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|| Utc::now() - Duration::hours(1))
    }

    pub fn to_datetime(&self) -> DateTime<Utc> {
        self.to
            .as_ref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(Utc::now)
    }

    pub fn max_events(&self) -> usize {
        self.max_events.unwrap_or(1000)
    }

    pub fn max_frames(&self) -> usize {
        self.max_frames.unwrap_or(500)
    }

    /// Convert optional bounds into a bounded `TimeWindow`.
    ///
    /// Mirrors `TimeRangeQuery::to_time_window` but uses `default_lookback`
    /// to control the from-fallback (timeline default is `Duration::hours(1)`
    /// matching pre-existing `from_datetime()` semantics).
    ///
    /// # Errors
    /// - [`TimeWindowError::ParseFailed`] if `from` or `to` is not RFC3339.
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
