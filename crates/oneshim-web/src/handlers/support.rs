use crate::services::support_service::SupportDiagnosticsQueryService;
use crate::services::web_contexts::SupportDiagnosticsContext;
use axum::{extract::State, Json};
use oneshim_api_contracts::support::DiagnosticsBundleDto;
#[cfg(test)]
use oneshim_api_contracts::support::DiagnosticsHealthDto;

pub async fn get_diagnostics(
    State(context): State<SupportDiagnosticsContext>,
) -> Json<DiagnosticsBundleDto> {
    Json(
        SupportDiagnosticsQueryService::new(context)
            .get_diagnostics()
            .await,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_health_serializes() {
        let dto = DiagnosticsHealthDto {
            storage_ok: true,
            storage_error: None,
            frames_dir_configured: true,
            frames_dir_path: Some("/tmp/frames".to_string()),
            frames_dir_exists: Some(true),
            config_manager_configured: true,
            automation_controller_configured: true,
            update_control_configured: false,
        };

        let json = serde_json::to_string(&dto).unwrap();
        assert!(json.contains("frames_dir_configured"));
        assert!(json.contains("automation_controller_configured"));
    }
}
