use axum::extract::State;
use axum::Json;
use oneshim_api_contracts::bug_report::{BugReportBundleDto, CreateBugReportRequest};

use crate::error::ApiError;
use crate::services::bug_report_service::BugReportService;
use crate::services::web_contexts::BugReportContext;

pub async fn create_bug_report_with_params(
    State(ctx): State<BugReportContext>,
    Json(params): Json<CreateBugReportRequest>,
) -> Result<Json<BugReportBundleDto>, ApiError> {
    let latest = ctx.latest.clone();
    let service = BugReportService::new(ctx);
    let bundle = service
        .create_report(params.include_logs, params.pii_level)
        .await?;

    {
        let mut guard = latest.write();
        *guard = Some(bundle.clone());
    }

    Ok(Json(bundle))
}

pub async fn get_latest_bug_report(
    State(ctx): State<BugReportContext>,
) -> Result<Json<BugReportBundleDto>, ApiError> {
    let guard = ctx.latest.read();
    match guard.as_ref() {
        Some(bundle) => Ok(Json(bundle.clone())),
        None => Err(ApiError::NotFound(
            "no bug report generated yet".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_request_deserializes_defaults() {
        let req: CreateBugReportRequest = serde_json::from_str("{}").unwrap();
        assert!(req.include_logs);
        assert!(req.pii_level.is_none());
    }

    #[test]
    fn create_request_with_params() {
        let req: CreateBugReportRequest =
            serde_json::from_str(r#"{"include_logs":false,"pii_level":"strict"}"#).unwrap();
        assert!(!req.include_logs);
        assert_eq!(req.pii_level.as_deref(), Some("strict"));
    }
}
