use oneshim_core::config::PiiFilterLevel;
use serde::{Deserialize, Serialize};

use crate::support::{DiagnosticsBundleDto, RuntimeLogSnapshotDto};

/// Complete bug report bundle for export/sharing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BugReportBundleDto {
    pub bug_id: String,
    pub diagnostics: DiagnosticsBundleDto,
    pub system: SystemInfoDto,
    pub connection: ConnectionStatusDto,
    pub runtime_logs: Option<RuntimeLogSnapshotDto>,
    pub pii_filter_level: PiiFilterLevel,
}

/// System hardware and software information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemInfoDto {
    pub app_version: String,
    pub os_name: String,
    pub os_version: String,
    pub arch: String,
    pub runtime: String,
    pub cpu_count: usize,
    pub memory_total_mb: u64,
    pub memory_available_mb: u64,
    pub uptime_seconds: u64,
}

/// Server connection status snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionStatusDto {
    pub server_reachable: bool,
    pub last_sync_at: Option<String>,
    pub grpc_enabled: bool,
    pub websocket_connected: bool,
}

/// Request parameters for creating a bug report.
#[derive(Debug, Deserialize)]
pub struct CreateBugReportRequest {
    #[serde(default = "default_include_logs")]
    pub include_logs: bool,
    #[serde(default)]
    pub pii_level: Option<String>,
}

fn default_include_logs() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_info_serializes() {
        let info = SystemInfoDto {
            app_version: "0.4.16".to_string(),
            os_name: "macOS".to_string(),
            os_version: "15.4".to_string(),
            arch: "aarch64".to_string(),
            runtime: "tauri-desktop".to_string(),
            cpu_count: 10,
            memory_total_mb: 16384,
            memory_available_mb: 8192,
            uptime_seconds: 3600,
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("\"cpu_count\":10"));
    }

    #[test]
    fn connection_status_serializes() {
        let status = ConnectionStatusDto {
            server_reachable: true,
            last_sync_at: Some("2026-04-03T10:00:00Z".to_string()),
            grpc_enabled: false,
            websocket_connected: false,
        };
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("server_reachable"));
    }

    #[test]
    fn create_request_defaults() {
        let req: CreateBugReportRequest = serde_json::from_str("{}").unwrap();
        assert!(req.include_logs);
        assert!(req.pii_level.is_none());
    }

    #[test]
    fn pii_filter_level_serde_matches_debug() {
        use oneshim_core::config::PiiFilterLevel;
        let level = PiiFilterLevel::Standard;
        let json = serde_json::to_value(level).unwrap();
        assert_eq!(json.as_str().unwrap(), format!("{level:?}"));
    }
}
