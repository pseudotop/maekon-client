use crate::error::ApiError;
pub(crate) use crate::services::settings_assembler::is_masked_key;
use crate::services::settings_config_mutation::apply_settings_fields_to_config;
use maekon_api_contracts::settings::AppSettings;
use maekon_core::config::{validate_integration_auth_token_strength, AppConfig};

pub(crate) fn apply_settings_to_config(
    config: &mut AppConfig,
    settings: &AppSettings,
) -> Result<(), ApiError> {
    if settings.allow_external {
        let token = config
            .web
            .integration_auth_token
            .as_deref()
            .unwrap_or_default();
        validate_integration_auth_token_strength(token).map_err(ApiError::BadRequest)?;
    }
    apply_settings_fields_to_config(config, settings)
}

#[cfg(test)]
mod tests_fixtures;
#[cfg(test)]
mod tests_mapping;
#[cfg(test)]
mod tests_update;
#[cfg(test)]
mod tests_validation;
