use async_trait::async_trait;
use chrono::{DateTime, Utc};
use oneshim_core::error::CoreError;
use oneshim_core::models::integration::{
    IntegrationAuthContext, IntegrationAuthProfileKind, IntegrationAuthScheme,
    IntegrationAuthStatus, IntegrationAuthStatusKind, IntegrationCapabilityScope,
    IntegrationDeviceAuthorizationFlow,
};
use oneshim_core::ports::integration::IntegrationAuthPort;

pub struct StaticIntegrationAuthPort {
    context: IntegrationAuthContext,
}

impl StaticIntegrationAuthPort {
    pub fn new(context: IntegrationAuthContext) -> Self {
        Self { context }
    }
}

#[async_trait]
impl IntegrationAuthPort for StaticIntegrationAuthPort {
    async fn resolve_session_auth(
        &self,
        _requested_scopes: &[IntegrationCapabilityScope],
        resource_indicator: Option<&str>,
    ) -> Result<IntegrationAuthContext, CoreError> {
        let mut context = self.context.clone();
        if context.resource_indicator.is_none() {
            context.resource_indicator = resource_indicator.map(str::to_string);
        }
        Ok(context)
    }

    async fn current_auth_status(&self) -> Result<IntegrationAuthStatus, CoreError> {
        Ok(IntegrationAuthStatus {
            profile_kind: IntegrationAuthProfileKind::EnvToken,
            status: IntegrationAuthStatusKind::Ready,
            interactive: false,
            authenticated: true,
            expires_at: self.context.expires_at,
            resource_indicator: self.context.resource_indicator.clone(),
            pending_flow: None,
            message: None,
        })
    }

    async fn start_device_authorization(
        &self,
        _requested_scopes: &[IntegrationCapabilityScope],
        _resource_indicator: Option<&str>,
    ) -> Result<IntegrationDeviceAuthorizationFlow, CoreError> {
        Err(CoreError::InvalidArguments {
            code: oneshim_core::error_codes::ValidationCode::InvalidArguments,
            message: "static integration auth does not support device authorization".to_string(),
        })
    }

    async fn poll_device_authorization(
        &self,
        _flow_id: &str,
    ) -> Result<IntegrationAuthStatus, CoreError> {
        self.current_auth_status().await
    }

    async fn cancel_device_authorization(&self, _flow_id: &str) -> Result<(), CoreError> {
        Err(CoreError::InvalidArguments {
            code: oneshim_core::error_codes::ValidationCode::InvalidArguments,
            message: "static integration auth does not support device authorization".to_string(),
        })
    }

    async fn reset_auth_state(&self) -> Result<(), CoreError> {
        Err(CoreError::InvalidArguments {
            code: oneshim_core::error_codes::ValidationCode::InvalidArguments,
            message: "static integration auth does not support auth reset".to_string(),
        })
    }
}

pub struct EnvIntegrationAuthPort {
    token_env_var: String,
    scheme: IntegrationAuthScheme,
    expires_at: Option<DateTime<Utc>>,
    resource_indicator: Option<String>,
}

impl EnvIntegrationAuthPort {
    pub fn new(
        token_env_var: impl Into<String>,
        scheme: IntegrationAuthScheme,
        expires_at: Option<DateTime<Utc>>,
        resource_indicator: Option<String>,
    ) -> Self {
        Self {
            token_env_var: token_env_var.into(),
            scheme,
            expires_at,
            resource_indicator,
        }
    }

    fn read_token(&self) -> Option<String> {
        std::env::var(&self.token_env_var)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    }
}

#[async_trait]
impl IntegrationAuthPort for EnvIntegrationAuthPort {
    async fn resolve_session_auth(
        &self,
        _requested_scopes: &[IntegrationCapabilityScope],
        resource_indicator: Option<&str>,
    ) -> Result<IntegrationAuthContext, CoreError> {
        let access_token = self.read_token().ok_or_else(|| CoreError::Auth {
            code: oneshim_core::error_codes::AuthCode::Failed,
            message: format!(
                "integration access token env var `{}` is not configured.",
                self.token_env_var
            ),
        })?;

        Ok(IntegrationAuthContext {
            access_token,
            scheme: self.scheme.clone(),
            expires_at: self.expires_at,
            resource_indicator: self
                .resource_indicator
                .clone()
                .or_else(|| resource_indicator.map(str::to_string)),
        })
    }

    async fn current_auth_status(&self) -> Result<IntegrationAuthStatus, CoreError> {
        let authenticated = self.read_token().is_some();
        Ok(IntegrationAuthStatus {
            profile_kind: IntegrationAuthProfileKind::EnvToken,
            status: if authenticated {
                IntegrationAuthStatusKind::Ready
            } else {
                IntegrationAuthStatusKind::Unauthenticated
            },
            interactive: false,
            authenticated,
            expires_at: self.expires_at,
            resource_indicator: self.resource_indicator.clone(),
            pending_flow: None,
            message: if authenticated {
                None
            } else {
                Some(format!(
                    "integration access token env var `{}` is not configured",
                    self.token_env_var
                ))
            },
        })
    }

    async fn start_device_authorization(
        &self,
        _requested_scopes: &[IntegrationCapabilityScope],
        _resource_indicator: Option<&str>,
    ) -> Result<IntegrationDeviceAuthorizationFlow, CoreError> {
        Err(CoreError::InvalidArguments {
            code: oneshim_core::error_codes::ValidationCode::InvalidArguments,
            message: "env-token integration auth does not support device authorization".to_string(),
        })
    }

    async fn poll_device_authorization(
        &self,
        _flow_id: &str,
    ) -> Result<IntegrationAuthStatus, CoreError> {
        self.current_auth_status().await
    }

    async fn cancel_device_authorization(&self, _flow_id: &str) -> Result<(), CoreError> {
        Err(CoreError::InvalidArguments {
            code: oneshim_core::error_codes::ValidationCode::InvalidArguments,
            message: "env-token integration auth does not support device authorization".to_string(),
        })
    }

    async fn reset_auth_state(&self) -> Result<(), CoreError> {
        Err(CoreError::InvalidArguments {
            code: oneshim_core::error_codes::ValidationCode::InvalidArguments,
            message: "env-token integration auth does not support auth reset".to_string(),
        })
    }
}
