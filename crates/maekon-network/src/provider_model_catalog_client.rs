use async_trait::async_trait;
use maekon_core::ports::provider_model_catalog::{
    ProviderModelCatalogError, ProviderModelCatalogPort, ProviderModelCatalogRequest,
    ProviderModelCatalogResponse,
};
use reqwest::header::{HeaderName, HeaderValue};

use crate::ANTHROPIC_API_VERSION;

#[derive(Debug, Clone, Default)]
pub struct ReqwestProviderModelCatalogClient;

impl ReqwestProviderModelCatalogClient {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ProviderModelCatalogPort for ReqwestProviderModelCatalogClient {
    async fn fetch_models(
        &self,
        request: ProviderModelCatalogRequest,
    ) -> Result<ProviderModelCatalogResponse, ProviderModelCatalogError> {
        let client = reqwest::Client::builder()
            .timeout(request.timeout)
            .build()
            .map_err(|error| ProviderModelCatalogError::ClientBuild(error.to_string()))?;

        let mut builder = client.get(&request.endpoint);
        for header in request.headers {
            let name = HeaderName::from_bytes(header.name.as_bytes()).map_err(|error| {
                ProviderModelCatalogError::InvalidHeader(format!("{}: {error}", header.name))
            })?;
            let value = HeaderValue::from_str(&header.value).map_err(|error| {
                ProviderModelCatalogError::InvalidHeader(format!("{}: {error}", header.name))
            })?;
            builder = builder.header(name, value);
        }

        let response = builder
            .send()
            .await
            .map_err(|error| ProviderModelCatalogError::Request(error.to_string()))?;
        let status = response.status().as_u16();
        let body = response
            .text()
            .await
            .map_err(|error| ProviderModelCatalogError::ResponseBody(error.to_string()))?;

        Ok(ProviderModelCatalogResponse { status, body })
    }
}

pub fn anthropic_version_header() -> (&'static str, &'static str) {
    ("anthropic-version", ANTHROPIC_API_VERSION)
}
