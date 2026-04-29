use oneshim_api_contracts::integration::{
    IntegrationDeviceAuthorizationCommandResult, IntegrationInboxActionResponse,
    IntegrationInboxDismissRequest, IntegrationInboxRefreshResponse, IntegrationInboxResponse,
};
use oneshim_core::models::integration::default_integration_runtime_scopes;

use crate::error::ApiError;
use crate::services::integration_assembler::map_prompt;
use crate::services::web_contexts::IntegrationWebContext;

use super::{INTEGRATION_INBOX_ACTION_SCHEMA_VERSION, INTEGRATION_INBOX_SCHEMA_VERSION};

#[derive(Clone)]
pub struct IntegrationInboxCommandService {
    pub(super) ctx: IntegrationWebContext,
}

impl IntegrationInboxCommandService {
    pub fn new(ctx: IntegrationWebContext) -> Self {
        Self { ctx }
    }

    pub async fn list_inbox(&self) -> Result<IntegrationInboxResponse, ApiError> {
        let prompts = require_inbox(&self.ctx)?
            .list_pending()
            .await?
            .into_iter()
            .map(map_prompt)
            .collect::<Vec<_>>();
        let pending_count = prompts.len();

        Ok(IntegrationInboxResponse {
            schema_version: INTEGRATION_INBOX_SCHEMA_VERSION.to_string(),
            prompts,
            pending_count,
        })
    }

    pub async fn refresh_inbox(&self) -> Result<IntegrationInboxRefreshResponse, ApiError> {
        Ok(IntegrationInboxRefreshResponse {
            schema_version: INTEGRATION_INBOX_SCHEMA_VERSION.to_string(),
            fetched_count: require_inbox(&self.ctx)?.refresh().await?,
        })
    }

    pub async fn acknowledge_inbox_prompt(
        &self,
        prompt_id: &str,
    ) -> Result<IntegrationInboxActionResponse, ApiError> {
        require_inbox(&self.ctx)?.acknowledge(prompt_id).await?;
        Ok(IntegrationInboxActionResponse {
            schema_version: INTEGRATION_INBOX_ACTION_SCHEMA_VERSION.to_string(),
            prompt_id: prompt_id.to_string(),
            status: "acknowledged".to_string(),
        })
    }

    pub async fn dismiss_inbox_prompt(
        &self,
        prompt_id: &str,
        request: IntegrationInboxDismissRequest,
    ) -> Result<IntegrationInboxActionResponse, ApiError> {
        require_inbox(&self.ctx)?
            .dismiss(prompt_id, request.reason)
            .await?;
        Ok(IntegrationInboxActionResponse {
            schema_version: INTEGRATION_INBOX_ACTION_SCHEMA_VERSION.to_string(),
            prompt_id: prompt_id.to_string(),
            status: "dismissed".to_string(),
        })
    }
}

#[derive(Clone)]
pub struct IntegrationAuthCommandService {
    pub(super) ctx: IntegrationWebContext,
}

impl IntegrationAuthCommandService {
    pub fn new(ctx: IntegrationWebContext) -> Self {
        Self { ctx }
    }

    pub async fn get_auth_status(
        &self,
    ) -> Result<oneshim_core::models::integration::IntegrationAuthStatus, ApiError> {
        Ok(require_auth(&self.ctx)?.current_auth_status().await?)
    }

    pub async fn start_device_authorization(
        &self,
    ) -> Result<IntegrationDeviceAuthorizationCommandResult, ApiError> {
        let auth = require_auth(&self.ctx)?;
        let flow = auth
            .start_device_authorization(&default_integration_runtime_scopes(), None)
            .await?;
        let auth_status = auth.current_auth_status().await?;
        Ok(IntegrationDeviceAuthorizationCommandResult {
            auth_status,
            flow: Some(flow),
        })
    }

    pub async fn poll_device_authorization(
        &self,
        flow_id: &str,
    ) -> Result<IntegrationDeviceAuthorizationCommandResult, ApiError> {
        let auth_status = require_auth(&self.ctx)?
            .poll_device_authorization(flow_id)
            .await?;
        Ok(IntegrationDeviceAuthorizationCommandResult {
            flow: auth_status.pending_flow.clone(),
            auth_status,
        })
    }

    pub async fn cancel_device_authorization(
        &self,
        flow_id: &str,
    ) -> Result<IntegrationDeviceAuthorizationCommandResult, ApiError> {
        let auth = require_auth(&self.ctx)?;
        auth.cancel_device_authorization(flow_id).await?;
        let auth_status = auth.current_auth_status().await?;
        Ok(IntegrationDeviceAuthorizationCommandResult {
            flow: auth_status.pending_flow.clone(),
            auth_status,
        })
    }

    pub async fn reset_auth_state(
        &self,
    ) -> Result<IntegrationDeviceAuthorizationCommandResult, ApiError> {
        let auth = require_auth(&self.ctx)?;
        auth.reset_auth_state().await?;
        let auth_status = auth.current_auth_status().await?;
        Ok(IntegrationDeviceAuthorizationCommandResult {
            flow: auth_status.pending_flow.clone(),
            auth_status,
        })
    }
}

fn require_inbox(
    context: &IntegrationWebContext,
) -> Result<std::sync::Arc<dyn oneshim_core::ports::integration::IntegrationInboxPort>, ApiError> {
    context.inbox.clone().ok_or_else(|| {
        ApiError::ServiceUnavailable("Integration inbox runtime is not configured.".to_string())
    })
}

fn require_auth(
    context: &IntegrationWebContext,
) -> Result<std::sync::Arc<dyn oneshim_core::ports::integration::IntegrationAuthPort>, ApiError> {
    context.auth.clone().ok_or_else(|| {
        ApiError::ServiceUnavailable("Integration auth runtime is not configured.".to_string())
    })
}
