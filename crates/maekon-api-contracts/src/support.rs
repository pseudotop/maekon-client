use crate::automation::AuditEntryDto;
use crate::settings::{AppSettings, StorageStats};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DiagnosticsHealthDto {
    pub storage_ok: bool,
    pub storage_error: Option<String>,
    pub frames_dir_configured: bool,
    pub frames_dir_path: Option<String>,
    pub frames_dir_exists: Option<bool>,
    pub config_manager_configured: bool,
    pub automation_controller_configured: bool,
    pub update_control_configured: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ProviderCliDiagnosticSummaryDto {
    pub surface_id: String,
    pub tool_id: Option<String>,
    pub candidate_name: Option<String>,
    pub executable_hint: Option<String>,
    pub readiness: String,
    pub availability: String,
    pub dependency_status: Option<String>,
    pub status_reason: Option<String>,
    pub env_refresh_required: bool,
}

impl From<maekon_core::models::provider_cli_diagnostics::ProviderCliDiagnosticSummary>
    for ProviderCliDiagnosticSummaryDto
{
    fn from(
        value: maekon_core::models::provider_cli_diagnostics::ProviderCliDiagnosticSummary,
    ) -> Self {
        Self {
            surface_id: value.surface_id,
            tool_id: value.tool_id,
            candidate_name: value.candidate_name,
            executable_hint: value.executable_hint,
            readiness: value.readiness,
            availability: value.availability,
            dependency_status: value.dependency_status,
            status_reason: value.status_reason,
            env_refresh_required: value.env_refresh_required,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DiagnosticsBundleDto {
    pub schema_version: String,
    pub generated_at: String,
    pub health: DiagnosticsHealthDto,
    pub settings_snapshot: AppSettings,
    pub storage_stats: Option<StorageStats>,
    #[serde(default)]
    pub provider_cli: Vec<ProviderCliDiagnosticSummaryDto>,
    pub recent_audit_entries: Vec<AuditEntryDto>,
    pub recent_policy_events: Vec<AuditEntryDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RuntimeLogSnapshotDto {
    pub generated_at: String,
    pub log_dir: String,
    pub log_file: Option<String>,
    pub line_count: usize,
    pub recent_text: String,
}

/// #7918: local diagnostics snapshot of the desktop agent's OWN process
/// resource usage (RSS + CPU) plus the provisional resource budget it is
/// measured against — the read side of the "<2% CPU, you won't notice it
/// running" claim made measurable.
///
/// LOCAL ONLY: populated by sampling the current process via sysinfo and
/// returned to the local dashboard / bug-report surface. No value here is
/// egressed (ADR-016) — this is the resource-usage sibling of
/// `RuntimeLogSnapshotDto`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ResourceUsageSnapshotDto {
    pub generated_at: String,
    /// Measured self RSS in bytes. `0` when `measured` is false.
    pub rss_bytes: u64,
    /// Measured self CPU usage (%, multi-core aggregate — may exceed 100%).
    /// `0.0` when `measured` is false.
    pub cpu_percent: f32,
    /// Provisional RSS enforcement ceiling (bytes) — the budget SSOT value.
    pub rss_budget_bytes: u64,
    /// Provisional CPU enforcement ceiling (%) — the budget SSOT value.
    pub cpu_budget_percent: f32,
    /// True when the measured RSS is within `rss_budget_bytes`.
    pub rss_within_budget: bool,
    /// True when the measured CPU is within `cpu_budget_percent`.
    pub cpu_within_budget: bool,
    /// False when the platform could not measure resource usage (unsupported
    /// OS or restricted process visibility). Consumers should render "n/a"
    /// rather than "0" in that case.
    pub measured: bool,
}
