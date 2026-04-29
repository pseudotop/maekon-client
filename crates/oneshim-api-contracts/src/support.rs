use crate::automation::AuditEntryDto;
use crate::settings::{AppSettings, StorageStats};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticsBundleDto {
    pub schema_version: String,
    pub generated_at: String,
    pub health: DiagnosticsHealthDto,
    pub settings_snapshot: AppSettings,
    pub storage_stats: Option<StorageStats>,
    pub recent_audit_entries: Vec<AuditEntryDto>,
    pub recent_policy_events: Vec<AuditEntryDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeLogSnapshotDto {
    pub generated_at: String,
    pub log_dir: String,
    pub log_file: Option<String>,
    pub line_count: usize,
    pub recent_text: String,
}
