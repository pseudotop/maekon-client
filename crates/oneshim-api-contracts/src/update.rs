use chrono::Utc;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum UpdatePhase {
    Idle,
    Checking,
    PendingApproval,
    Downloading,
    ReadyToInstall,
    Installing,
    Updated,
    Deferred,
    Error,
    /// Phase 4 D11: the automatic health probe detected two consecutive
    /// failed boots without a self-healthy marker and restored the previous
    /// binary. UI renders `UpdateStatus.rollback` (a `RollbackInfo`) when
    /// this variant is active.
    RolledBack,
}

/// Reason the updater escalated a post-install failure to an automatic
/// rollback. Additive enum — new reasons can be appended without breaking
/// existing consumers (snake_case serde).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RollbackReason {
    /// D11 health probe counted two consecutive failed boots without a
    /// self-healthy marker and escalated to rollback.
    RepeatedStartupFailure,
}

/// Metadata describing a completed post-install rollback. Populated on
/// `UpdateStatus` when the phase transitions to `UpdatePhase::RolledBack`.
/// All timestamp fields are RFC3339 UTC strings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RollbackInfo {
    pub from_version: String,
    /// RFC3339 UTC `published_at` of the rolled-from release (if known).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_published_at: Option<String>,
    pub to_version: String,
    /// RFC3339 UTC `published_at` of the rolled-to release (if known).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_published_at: Option<String>,
    pub reason: RollbackReason,
    /// RFC3339 UTC timestamp at which the rollback completed.
    pub rolled_back_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct PendingUpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub release_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published_at: Option<String>,
    pub download_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_notes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub download_size_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct DownloadProgress {
    pub bytes_downloaded: u64,
    pub total_bytes: u64,
    pub percent: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateStatus {
    pub enabled: bool,
    pub auto_install: bool,
    pub phase: UpdatePhase,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending: Option<PendingUpdateInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub download_progress: Option<DownloadProgress>,
    /// Phase 4 D11: populated when `phase == UpdatePhase::RolledBack`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rollback: Option<RollbackInfo>,
    pub revision: u64,
    pub updated_at: String,
}

impl Default for UpdateStatus {
    fn default() -> Self {
        Self {
            enabled: false,
            auto_install: false,
            phase: UpdatePhase::Idle,
            message: None,
            pending: None,
            download_progress: None,
            rollback: None,
            revision: 0,
            updated_at: Utc::now().to_rfc3339(),
        }
    }
}

impl UpdateStatus {
    pub fn touch(&mut self) {
        self.revision = self.revision.saturating_add(1);
        self.updated_at = Utc::now().to_rfc3339();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum UpdateAction {
    Approve,
    Defer,
    CheckNow,
}

#[derive(Debug, Deserialize)]
pub struct UpdateActionRequest {
    pub action: UpdateAction,
}

#[derive(Debug, Serialize)]
pub struct UpdateActionResponse {
    pub accepted: bool,
    pub status: UpdateStatus,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_update_phase() {
        for phase in [
            UpdatePhase::Idle,
            UpdatePhase::Checking,
            UpdatePhase::PendingApproval,
            UpdatePhase::Downloading,
            UpdatePhase::ReadyToInstall,
            UpdatePhase::Installing,
            UpdatePhase::Updated,
            UpdatePhase::Deferred,
            UpdatePhase::Error,
            UpdatePhase::RolledBack,
        ] {
            let json = serde_json::to_string(&phase).unwrap();
            let decoded: UpdatePhase = serde_json::from_str(&json).unwrap();
            assert_eq!(phase, decoded);
        }
    }

    #[test]
    fn rollback_reason_uses_snake_case_serde() {
        let json = serde_json::to_string(&RollbackReason::RepeatedStartupFailure).unwrap();
        assert_eq!(json, r#""repeated_startup_failure""#);
        let decoded: RollbackReason = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, RollbackReason::RepeatedStartupFailure);
    }

    #[test]
    fn round_trip_rollback_info() {
        let original = RollbackInfo {
            from_version: "0.5.0-rc.1".to_string(),
            from_published_at: Some("2026-05-01T00:00:00Z".to_string()),
            to_version: "0.4.40".to_string(),
            to_published_at: Some("2026-04-20T00:00:00Z".to_string()),
            reason: RollbackReason::RepeatedStartupFailure,
            rolled_back_at: "2026-05-02T12:34:56Z".to_string(),
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: RollbackInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn rollback_info_dates_skipped_when_none() {
        let original = RollbackInfo {
            from_version: "0.5.0-rc.1".to_string(),
            from_published_at: None,
            to_version: "0.4.40".to_string(),
            to_published_at: None,
            reason: RollbackReason::RepeatedStartupFailure,
            rolled_back_at: "2026-05-02T12:34:56Z".to_string(),
        };
        let json = serde_json::to_string(&original).unwrap();
        assert!(!json.contains("from_published_at"));
        assert!(!json.contains("to_published_at"));
    }

    #[test]
    fn round_trip_pending_update_info() {
        let original = PendingUpdateInfo {
            current_version: "0.4.32".to_string(),
            latest_version: "0.4.33".to_string(),
            release_url: "https://github.com/example/releases/v0.4.33".to_string(),
            release_name: Some("v0.4.33".to_string()),
            published_at: Some("2026-04-11T00:00:00Z".to_string()),
            download_url: "https://github.com/example/releases/v0.4.33/app.tar.gz".to_string(),
            release_notes: Some("Bug fixes and performance improvements.".to_string()),
            download_size_bytes: Some(12_345_678),
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: PendingUpdateInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn round_trip_download_progress() {
        let original = DownloadProgress {
            bytes_downloaded: 6_172_839,
            total_bytes: 12_345_678,
            percent: 50.0,
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: DownloadProgress = serde_json::from_str(&json).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn round_trip_update_action() {
        for action in [
            UpdateAction::Approve,
            UpdateAction::Defer,
            UpdateAction::CheckNow,
        ] {
            let json = serde_json::to_string(&action).unwrap();
            let decoded: UpdateAction = serde_json::from_str(&json).unwrap();
            assert_eq!(action, decoded);
        }
    }

    #[test]
    fn pending_update_info_optional_fields_skipped_when_none() {
        let original = PendingUpdateInfo {
            current_version: "0.4.32".to_string(),
            latest_version: "0.4.33".to_string(),
            release_url: "https://github.com/example/releases/v0.4.33".to_string(),
            release_name: None,
            published_at: None,
            download_url: "https://example.com/app.tar.gz".to_string(),
            release_notes: None,
            download_size_bytes: None,
        };
        let json = serde_json::to_string(&original).unwrap();
        assert!(!json.contains("release_name"));
        assert!(!json.contains("release_notes"));
        assert!(!json.contains("download_size_bytes"));
        let decoded: PendingUpdateInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(original, decoded);
    }
}
