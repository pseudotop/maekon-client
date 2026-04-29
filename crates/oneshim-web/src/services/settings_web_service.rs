use oneshim_api_contracts::settings::AppSettings;

use crate::error::ApiError;
use crate::services::settings_update_flow::SettingsUpdateFlow;
use crate::services::settings_validation::validate_settings_input;
use crate::services::web_contexts::SettingsWebContext;

#[derive(Clone)]
pub struct SettingsCommandService {
    ctx: SettingsWebContext,
}

impl SettingsCommandService {
    pub fn new(ctx: SettingsWebContext) -> Self {
        Self { ctx }
    }

    pub async fn update_settings(&self, settings: &AppSettings) -> Result<(), ApiError> {
        validate_settings_input(settings)?;

        if let Some(ref config_manager) = self.ctx.config_manager {
            SettingsUpdateFlow::new(
                config_manager.clone(),
                self.ctx.default_secret_backend_kind,
                self.ctx.secret_store.clone(),
                self.ctx.secret_stores.clone(),
                self.ctx.audit_logger.clone(),
            )
            .apply(settings)
            .await?;
        }

        Ok(())
    }
}
