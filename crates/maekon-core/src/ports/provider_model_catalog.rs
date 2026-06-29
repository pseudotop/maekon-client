//! Provider model catalog discovery transport port.

use std::net::SocketAddr;
use std::time::Duration;

use async_trait::async_trait;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderModelCatalogHeader {
    pub name: String,
    pub value: String,
}

impl ProviderModelCatalogHeader {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderModelCatalogRequest {
    pub endpoint: String,
    pub headers: Vec<ProviderModelCatalogHeader>,
    pub timeout: Duration,
    /// #6902: `SocketAddr`s of the endpoint host the caller already resolved and verified, for
    /// DNS-rebinding hardening. When non-empty, the transport does not re-resolve the host and
    /// pins to these addresses (`resolve_to_addrs`) — closing the TOCTOU rebinding window between
    /// the SSRF guard's (maekon-web) host resolution and the transport's re-resolution. An empty
    /// vector means no pin (preserves existing behavior).
    pub resolved_addrs: Vec<SocketAddr>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderModelCatalogResponse {
    pub status: u16,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ProviderModelCatalogError {
    #[error("failed to build model discovery client: {0}")]
    ClientBuild(String),
    #[error("invalid model discovery header: {0}")]
    InvalidHeader(String),
    #[error("model discovery request failed: {0}")]
    Request(String),
    #[error("failed to read model discovery response: {0}")]
    ResponseBody(String),
}

#[async_trait]
pub trait ProviderModelCatalogPort: Send + Sync {
    async fn fetch_models(
        &self,
        request: ProviderModelCatalogRequest,
    ) -> Result<ProviderModelCatalogResponse, ProviderModelCatalogError>;
}
