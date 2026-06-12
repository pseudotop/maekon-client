use std::future::Future;
use std::pin::Pin;

use maekon_api_contracts::support::ProviderCliDiagnosticSummaryDto;

pub type ProviderCliDiagnosticsFuture<'a> =
    Pin<Box<dyn Future<Output = Vec<ProviderCliDiagnosticSummaryDto>> + Send + 'a>>;

pub trait ProviderCliDiagnosticsProvider: Send + Sync {
    fn provider_cli_diagnostics(&self) -> ProviderCliDiagnosticsFuture<'_>;
}
