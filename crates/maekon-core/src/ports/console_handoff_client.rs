//! Authenticated pending handoff transport (#9628).
//!
//! The call takes no arguments. Actor and organization come only from the
//! shared Rust-side bearer, so the WebView cannot claim either identity or pass
//! a credential through a URL.

use async_trait::async_trait;

use crate::error::CoreError;
use crate::models::console_handoff::ConsoleHandoffIssue;

#[async_trait]
pub trait ConsoleHandoffClient: Send + Sync {
    async fn issue_console_handoff(&self) -> Result<ConsoleHandoffIssue, CoreError>;
}
