use crate::error::ApiError;
pub(crate) use crate::services::settings_assembler::is_masked_key;
use crate::services::settings_config_mutation::apply_settings_fields_to_config;
use oneshim_api_contracts::settings::AppSettings;
use oneshim_core::config::AppConfig;

pub(crate) fn apply_settings_to_config(
    config: &mut AppConfig,
    settings: &AppSettings,
) -> Result<(), ApiError> {
    if settings.allow_external
        && config
            .web
            .integration_auth_token
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_none()
    {
        return Err(ApiError::BadRequest(
            "allow_external requires web.integration_auth_token to be configured in config.json before enabling external access."
                .to_string(),
        ));
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
