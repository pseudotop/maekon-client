//! Context-home transport port (#9625, WD-02.2a).
//!
//! ## Why this is its own trait and not a method on `ApiClient`
//!
//! `ApiClient` has 13 implementers across the workspace — production adapters
//! (`HttpApiClient`, `GrpcApiAdapter`, `LocalApiClient`, `SyntheticUploadClient`)
//! plus test doubles. Adding a method there would force every one of them to
//! grow a stub for a call only the connected HTTP path can serve, and the
//! gRPC/local adapters would carry an `unimplemented!` that means nothing.
//! A narrow port keeps the obligation where the capability actually is.
//!
//! ## The boundary this port exists to hold
//!
//! Everything above this trait — the WebView, the IPC layer, logs, telemetry —
//! must never see the server bearer. The implementation reads it from the shared
//! `TokenManager` inside Rust and attaches it to the request; the token is not a
//! parameter here, so no caller can pass, capture, or forward one. The endpoint
//! likewise accepts no identity parameters: the server resolves actor and org
//! from the JWT alone, which is why `fetch_context_home` takes no arguments.

use async_trait::async_trait;

use crate::error::CoreError;
use crate::models::context_home::ContextHomeSnapshot;

#[async_trait]
pub trait ContextHomeClient: Send + Sync {
    /// Fetch the authenticated actor's context-home snapshot.
    ///
    /// Takes **no arguments on purpose**: actor and organization come from the
    /// JWT the transport already holds. A `user_id`/`organization_id` parameter
    /// here would make "request someone else's home" expressible, and the only
    /// thing standing between that and a leak would be a server-side check.
    ///
    /// # Errors
    /// Failure classes are distinguished so the caller can act differently
    /// (see `docs/guides/http-status-error-mapping.md`):
    /// - `auth.failed` — 401: the session is gone; re-login is the fix.
    /// - `policy.denied` — 403: authenticated but not permitted; re-login will
    ///   not help. Collapsing this into `auth.failed` would send the user to a
    ///   login screen that cannot resolve anything.
    /// - `network.timeout` / `service.unavailable` — transient; retry is sane.
    /// - `validation.*` — the body was absent, oversized, or unparsable.
    async fn fetch_context_home(&self) -> Result<ContextHomeSnapshot, CoreError>;
}
