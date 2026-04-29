use oneshim_api_contracts::settings::AppSettings;
use oneshim_core::config::CredentialBackendKind;
use oneshim_core::config_manager::ConfigManager;
use oneshim_core::ports::audit_log::AuditLogPort;
use oneshim_core::ports::secret_store::{SecretStore, SecretStoreSet};
use std::sync::Arc;

use crate::error::ApiError;
use crate::services::settings_policy_service::emit_policy_change_events;
use crate::services::settings_secret_persistence::persist_api_key_bindings;
use crate::services::settings_service::apply_settings_to_config;

#[derive(Clone)]
pub(crate) struct SettingsUpdateFlow {
    config_manager: ConfigManager,
    default_secret_backend_kind: CredentialBackendKind,
    secret_store: Option<Arc<dyn SecretStore>>,
    secret_stores: Option<SecretStoreSet>,
    audit_logger: Option<Arc<dyn AuditLogPort>>,
}

impl SettingsUpdateFlow {
    pub(crate) fn new(
        config_manager: ConfigManager,
        default_secret_backend_kind: CredentialBackendKind,
        secret_store: Option<Arc<dyn SecretStore>>,
        secret_stores: Option<SecretStoreSet>,
        audit_logger: Option<Arc<dyn AuditLogPort>>,
    ) -> Self {
        Self {
            config_manager,
            default_secret_backend_kind,
            secret_store,
            secret_stores,
            audit_logger,
        }
    }

    pub(crate) async fn apply(&self, settings: &AppSettings) -> Result<(), ApiError> {
        let previous_config = self.config_manager.get();
        let mut next_config = previous_config.clone();

        apply_settings_to_config(&mut next_config, settings)?;
        persist_api_key_bindings(
            &mut next_config,
            self.secret_store.clone(),
            self.secret_stores.as_ref(),
            self.default_secret_backend_kind,
        )
        .await?;

        next_config
            .ai_provider
            .validate_selected_remote_endpoints()
            .map_err(|error| ApiError::BadRequest(error.to_string()))?;

        self.config_manager
            .update(next_config.clone())
            .map_err(ApiError::from)?;

        emit_policy_change_events(self.audit_logger.clone(), &previous_config, &next_config);

        Ok(())
    }
}
