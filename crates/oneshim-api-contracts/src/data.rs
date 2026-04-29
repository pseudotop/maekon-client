use oneshim_core::types::{TimeWindow, TimeWindowError};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct DeleteRangeRequest {
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub data_types: Vec<String>,
}

impl DeleteRangeRequest {
    /// Construct a `TimeWindow` from the request's `from`/`to` string fields.
    ///
    /// Per Phase 1 iter-1 Q-10 option (b) + Phase 2 iter-1 C9 Option C: keeps
    /// external JSON shape (`from`, `to` keys) AND internal struct shape
    /// trivially. Frontend `DataSection.tsx` requires NO changes.
    ///
    /// # Errors
    /// - [`TimeWindowError::ParseFailed`] if `from` or `to` is not RFC3339.
    /// - [`TimeWindowError::InvertedBounds`] if parsed `start > end`.
    pub fn period(&self) -> Result<TimeWindow, TimeWindowError> {
        TimeWindow::from_rfc3339_pair(&self.from, &self.to)
    }
}

#[derive(Debug, Serialize)]
pub struct DeleteResult {
    pub success: bool,
    pub events_deleted: u64,
    pub frames_deleted: u64,
    pub metrics_deleted: u64,
    pub process_snapshots_deleted: u64,
    pub idle_periods_deleted: u64,
    pub message: String,
}

impl DeleteResult {
    pub fn empty() -> Self {
        Self {
            success: true,
            events_deleted: 0,
            frames_deleted: 0,
            metrics_deleted: 0,
            process_snapshots_deleted: 0,
            idle_periods_deleted: 0,
            message: String::new(),
        }
    }

    pub fn total(&self) -> u64 {
        self.events_deleted
            + self.frames_deleted
            + self.metrics_deleted
            + self.process_snapshots_deleted
            + self.idle_periods_deleted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delete_range_request_external_shape_preserved() {
        // Frontend sends from/to keys — no change required by the refactor.
        let json = r#"{"from":"2026-04-01T00:00:00Z","to":"2026-04-25T00:00:00Z","data_types":["frames"]}"#;
        let req: DeleteRangeRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.from, "2026-04-01T00:00:00Z");
        assert_eq!(req.to, "2026-04-25T00:00:00Z");
        assert_eq!(req.data_types, vec!["frames"]);
    }

    #[test]
    fn delete_range_request_period_accessor_returns_window() {
        let req = DeleteRangeRequest {
            from: "2026-04-01T00:00:00Z".to_string(),
            to: "2026-04-25T00:00:00Z".to_string(),
            data_types: vec!["frames".to_string()],
        };
        let window = req.period().unwrap();
        let expected =
            TimeWindow::from_rfc3339_pair("2026-04-01T00:00:00Z", "2026-04-25T00:00:00Z").unwrap();
        assert_eq!(window, expected);
    }

    #[test]
    fn delete_range_request_period_rejects_inverted_bounds() {
        let req = DeleteRangeRequest {
            from: "2026-04-25T00:00:00Z".to_string(),
            to: "2026-04-01T00:00:00Z".to_string(),
            data_types: vec![],
        };
        assert!(matches!(
            req.period(),
            Err(TimeWindowError::InvertedBounds { .. })
        ));
    }

    #[test]
    fn delete_range_request_period_rejects_invalid_rfc3339() {
        let req = DeleteRangeRequest {
            from: "not-a-date".to_string(),
            to: "2026-04-25T00:00:00Z".to_string(),
            data_types: vec![],
        };
        assert!(matches!(req.period(), Err(TimeWindowError::ParseFailed(_))));
    }
}
