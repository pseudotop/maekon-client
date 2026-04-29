//! gRPC client module — Consumer Contract (oneshim.client.v1).
//! ## Health Check
//! ```rust,ignore
//! let mut health = GrpcHealthClient::connect(config).await?;
//! if health.is_healthy().await {
//!     // server is reachable
//! }
//! ```

#[cfg(feature = "grpc")]
mod api_adapter;
#[cfg(feature = "grpc")]
mod auth_client;
#[cfg(feature = "grpc")]
mod config;
#[cfg(feature = "grpc")]
mod context_client;
#[cfg(feature = "grpc")]
mod error_mapping;
#[cfg(feature = "grpc")]
mod health_client;
#[cfg(feature = "grpc")]
mod session_client;
#[cfg(feature = "grpc")]
mod sse_adapter;
#[cfg(feature = "grpc")]
mod unified_client;

#[cfg(feature = "grpc")]
pub use api_adapter::GrpcApiAdapter;
#[cfg(feature = "grpc")]
pub use auth_client::GrpcAuthClient;
#[cfg(feature = "grpc")]
pub use config::GrpcConfig;
#[cfg(feature = "grpc")]
pub use context_client::GrpcContextClient;
#[cfg(feature = "grpc")]
pub use error_mapping::map_grpc_status_error;
#[cfg(feature = "grpc")]
pub use health_client::GrpcHealthClient;
#[cfg(feature = "grpc")]
pub use session_client::GrpcSessionClient;
#[cfg(feature = "grpc")]
pub use sse_adapter::GrpcSseAdapter;
#[cfg(feature = "grpc")]
pub use unified_client::{
    AuthResponse, FeedbackAction, SessionResponse, Streaming, SuggestionEvent, UnifiedClient,
    UploadBatchRequest, UploadBatchResponse,
};
