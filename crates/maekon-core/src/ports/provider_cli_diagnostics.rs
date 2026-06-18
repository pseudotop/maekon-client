//! Inbound port: provider CLI diagnostics provider.
//!
//! The local web dashboard's support/diagnostics surface consumes this port to
//! gather per-provider-surface CLI readiness. The implementation lives in the
//! `src-tauri` binary (snapshot-backed); `maekon-web` and `src-tauri` are
//! distinct crates, so this is a cross-crate contract that must originate in
//! `maekon-core` (ADR-001 §7). A thin re-export shim is kept in
//! `maekon-web::services::provider_cli_diagnostics` for source compatibility.
//!
//! The future is hand-rolled (`Pin<Box<dyn Future ...>>`) rather than
//! `#[async_trait]` so the borrow-scoped `'_` lifetime on the returned future is
//! explicit at the call sites that pin it.

use std::future::Future;
use std::pin::Pin;

use crate::models::provider_cli_diagnostics::ProviderCliDiagnosticSummary;

pub type ProviderCliDiagnosticsFuture<'a> =
    Pin<Box<dyn Future<Output = Vec<ProviderCliDiagnosticSummary>> + Send + 'a>>;

pub trait ProviderCliDiagnosticsProvider: Send + Sync {
    fn provider_cli_diagnostics(&self) -> ProviderCliDiagnosticsFuture<'_>;
}
