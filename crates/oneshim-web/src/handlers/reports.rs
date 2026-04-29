use axum::extract::{Query, State};
use axum::Json;
#[cfg(test)]
use oneshim_api_contracts::reports::{AppStat, DailyStat, ProductivityMetrics, ReportPeriod};
use oneshim_api_contracts::reports::{ReportQuery, ReportResponse};

use crate::error::ApiError;
use crate::services::reports_service::ReportQueryService;
use crate::services::web_contexts::StorageWebContext;

pub async fn generate_report(
    State(context): State<StorageWebContext>,
    Query(params): Query<ReportQuery>,
) -> Result<Json<ReportResponse>, ApiError> {
    Ok(Json(
        ReportQueryService::new(context)
            .generate_report(&params)
            .await?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_period_deserializes() {
        let json = r#""week""#;
        let period: ReportPeriod = serde_json::from_str(json).unwrap();
        assert_eq!(period, ReportPeriod::Week);

        let json = r#""month""#;
        let period: ReportPeriod = serde_json::from_str(json).unwrap();
        assert_eq!(period, ReportPeriod::Month);
    }

    #[test]
    fn report_response_serializes() {
        let response = ReportResponse {
            title: "Weekly Report".to_string(),
            from_date: "2024-01-23".to_string(),
            to_date: "2024-01-30".to_string(),
            days: 7,
            total_active_secs: 28800,
            total_idle_secs: 3600,
            total_captures: 100,
            total_events: 500,
            avg_cpu: 35.5,
            avg_memory: 68.2,
            daily_stats: vec![],
            app_stats: vec![],
            hourly_activity: vec![],
            productivity: ProductivityMetrics {
                score: 85.0,
                active_ratio: 80.0,
                peak_hour: 10,
                top_app: "VS Code".to_string(),
                trend: 5.5,
            },
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("Weekly Report"));
        assert!(json.contains("productivity"));
    }

    #[test]
    fn daily_stat_serializes() {
        let stat = DailyStat {
            date: "2024-01-30".to_string(),
            active_secs: 14400,
            idle_secs: 1800,
            captures: 50,
            events: 200,
            cpu_avg: 40.0,
            memory_avg: 70.0,
        };
        let json = serde_json::to_string(&stat).unwrap();
        assert!(json.contains("2024-01-30"));
        assert!(json.contains("14400"));
    }

    #[test]
    fn app_stat_serializes() {
        let stat = AppStat {
            name: "VS Code".to_string(),
            duration_secs: 7200,
            events: 150,
            captures: 30,
            percentage: 45.5,
        };
        let json = serde_json::to_string(&stat).unwrap();
        assert!(json.contains("VS Code"));
        assert!(json.contains("45.5"));
    }
}
