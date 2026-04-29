pub mod ai_models;
pub mod ai_provider_surfaces;
pub mod ai_session;
pub mod annotations;
pub mod audit_export;
pub mod automation;
pub mod automation_gui;
pub mod bug_report;
pub mod daily_digest;
pub mod dashboard;
pub mod digests;
pub mod recalibration;

pub mod backup;
pub mod coaching;
pub mod data;
pub mod events;
pub mod export;
pub mod external_grpc_live_config;
pub mod focus;
pub mod frames;
pub mod idle;
pub mod integration;
pub mod metrics;
pub mod onboarding;
pub mod playbooks;
pub mod pomodoro;
pub mod processes;
pub mod reports;
pub mod search;
pub mod semantic_search;
pub mod sessions;
pub mod settings;
pub mod stats;
pub mod stream;
pub mod suggestions;
pub mod support;
pub mod tags;
pub mod timeline;
pub mod tracking_schedule;
pub mod update;

pub use oneshim_api_contracts::common::{PaginatedResponse, PaginationMeta, TimeRangeQuery};

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};

    #[test]
    fn time_range_defaults() {
        let query = TimeRangeQuery {
            from: None,
            to: None,
            limit: None,
            offset: None,
            min_importance: None,
        };

        let now = Utc::now();
        assert!(query.from_datetime() < now);
        assert!(query.to_datetime() <= now + Duration::seconds(1));
        assert_eq!(query.limit_or_default(), 100);
        assert_eq!(query.offset_or_default(), 0);
    }

    #[test]
    fn time_range_custom() {
        let query = TimeRangeQuery {
            from: Some("2024-01-01T00:00:00Z".to_string()),
            to: Some("2024-01-02T00:00:00Z".to_string()),
            limit: Some(50),
            offset: Some(10),
            min_importance: None,
        };

        assert_eq!(query.limit_or_default(), 50);
        assert_eq!(query.offset_or_default(), 10);
        assert_eq!(
            query.from_datetime().to_rfc3339(),
            "2024-01-01T00:00:00+00:00"
        );
    }

    #[test]
    fn pagination_meta_has_more() {
        let meta = PaginationMeta {
            total: 100,
            offset: 0,
            limit: 50,
            has_more: true,
        };
        assert!(meta.has_more);
        assert_eq!(meta.total, 100);
    }
}
