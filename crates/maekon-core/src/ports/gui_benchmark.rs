use async_trait::async_trait;

use crate::error::CoreError;
use crate::models::gui::{GuiBenchmarkCase, GuiBenchmarkPortKind, GuiBenchmarkResult};

/// Contract-reserved adapter surface for OS-specific GUI benchmark runners.
///
/// This trait is named by the versioned `automation.gui.benchmark_harness.v1`
/// contract (`docs/contracts/gui-benchmark-harness.md`, "Domain types"): OS
/// launchers are expected to expose it and run the shared case catalog. No
/// in-tree implementation exists yet — the current harness machinery
/// (`src-tauri/src/windows_gui_session_benchmark.rs`) consumes the sibling
/// `GuiBenchmark*` model types directly.
///
/// Deliberately kept despite having zero implementations (#5592): deleting it
/// would silently break the published contract. Revise
/// `gui-benchmark-harness.v1` first if this surface is ever retired.
#[async_trait]
pub trait GuiBenchmarkAdapter: Send + Sync {
    fn adapter_name(&self) -> &str;

    fn port_kind(&self) -> GuiBenchmarkPortKind;

    async fn run_benchmark_case(
        &self,
        case: &GuiBenchmarkCase,
    ) -> Result<GuiBenchmarkResult, CoreError>;
}
