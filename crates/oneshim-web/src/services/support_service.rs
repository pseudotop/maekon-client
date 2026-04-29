use oneshim_api_contracts::support::DiagnosticsBundleDto;

use crate::services::settings_query_service::SettingsQueryService;
use crate::services::support_assembler::{
    assemble_diagnostics_bundle, assemble_diagnostics_health, to_audit_entry_dto,
    DiagnosticsHealthInput,
};
use crate::services::web_contexts::SupportDiagnosticsContext;

#[derive(Clone)]
pub struct SupportDiagnosticsQueryService {
    ctx: SupportDiagnosticsContext,
}

impl SupportDiagnosticsQueryService {
    pub fn new(ctx: SupportDiagnosticsContext) -> Self {
        Self { ctx }
    }

    pub async fn get_diagnostics(&self) -> DiagnosticsBundleDto {
        let settings_queries = SettingsQueryService::new(self.ctx.settings.clone());
        let settings_snapshot = settings_queries.get_settings();
        let (storage_stats, storage_error) = match settings_queries.get_storage_stats() {
            Ok(stats) => (Some(stats), None),
            Err(err) => (None, Some(err.to_string())),
        };

        let frames_dir_path = self
            .ctx
            .frames_dir
            .as_ref()
            .map(|path| path.display().to_string());
        let frames_dir_exists = self.ctx.frames_dir.as_ref().map(|path| path.exists());

        let health = assemble_diagnostics_health(DiagnosticsHealthInput {
            storage_error,
            frames_dir_path,
            frames_dir_exists,
            config_manager_configured: self.ctx.config_manager_configured,
            automation_controller_configured: self.ctx.automation_controller_configured,
            update_control_configured: self.ctx.update_control_configured,
        });

        let (recent_audit_entries, recent_policy_events) =
            if let Some(logger) = self.ctx.audit_logger.as_ref() {
                let recent_entries = logger.recent_entries(400).await;
                let audit_entries = recent_entries
                    .iter()
                    .take(50)
                    .cloned()
                    .map(to_audit_entry_dto)
                    .collect();
                let policy_entries = recent_entries
                    .into_iter()
                    .filter(|entry| entry.action_type.starts_with("policy."))
                    .take(50)
                    .map(to_audit_entry_dto)
                    .collect();
                (audit_entries, policy_entries)
            } else {
                (Vec::new(), Vec::new())
            };

        assemble_diagnostics_bundle(
            health,
            settings_snapshot,
            storage_stats,
            recent_audit_entries,
            recent_policy_events,
        )
    }
}
