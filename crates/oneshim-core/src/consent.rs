use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::error::CoreError;

pub const CURRENT_POLICY_VERSION: &str = "1.0.0";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConsentPermissions {
    // --- Tier 1 ---
    #[serde(default)]
    pub screen_capture: bool,
    #[serde(default)]
    pub ocr_processing: bool,
    #[serde(default)]
    pub telemetry: bool,
    #[serde(default)]
    pub process_monitoring: bool,
    #[serde(default)]
    pub input_activity: bool,

    // --- Tier 2 ---
    #[serde(default)]
    pub window_title_collection: bool,
    #[serde(default)]
    pub app_usage_analytics: bool,

    // --- Tier 3 ---
    #[serde(default)]
    pub clipboard_monitoring: bool,
    #[serde(default)]
    pub file_access_monitoring: bool,

    // --- Tier 4: Tiered Memory ---
    #[serde(default)]
    pub activity_pattern_learning: bool,

    // --- Tier 5: Cross-Device Sync ---
    /// Permits cross-device synchronization of activity data.
    /// GDPR Article 6 -- processing requires explicit consent for data
    /// transfer between devices, even when both are owned by the same user.
    #[serde(default)]
    pub cross_device_sync: bool,

    // --- Tier 6: Text Intelligence ---
    /// Permits extraction of full text content from focused UI elements.
    /// Required only when pii_extraction_level is set to Off.
    /// GDPR Article 6 -- explicit consent for processing text content
    /// that may contain personal data.
    #[serde(default)]
    pub full_text_extraction: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsentRecord {
    pub consent_id: String,
    pub version: String,
    pub granted_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    /// Timestamp recorded when the user revokes consent (GDPR Article 17 audit trail).
    #[serde(default)]
    pub revoked_at: Option<DateTime<Utc>>,
    /// Set to true after revocation to signal that queued data must be purged
    /// before the next upload cycle (GDPR Article 17 — right to erasure).
    #[serde(default)]
    pub data_deletion_requested: bool,
    pub permissions: ConsentPermissions,
    pub data_retention_days: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConsentStatus {
    NotGranted,
    Valid,
    Expired,
    UpdateRequired,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserDataExport {
    pub exported_at: DateTime<Utc>,
    pub consent: Option<ConsentRecord>,
    pub settings: serde_json::Value,
    pub event_count: u64,
    pub frame_count: u64,
    pub export_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeletionResult {
    pub deleted_at: DateTime<Utc>,
    pub events_deleted: u64,
    pub frames_deleted: u64,
    pub metrics_deleted: u64,
    pub settings_reset: bool,
    pub consent_revoked: bool,
}

// ConsentManager

pub struct ConsentManager {
    storage_path: PathBuf,
    current_consent: Option<ConsentRecord>,
    /// revoke_consent() 호출 후 데이터 소거가 완료되기 전까지 true를 유지한다.
    /// current_consent = None 이후에도 GDPR Article 17 신호가 소실되지 않도록
    /// 별도 in-memory 플래그로 관리한다.
    pending_deletion: bool,
}

impl ConsentManager {
    pub fn new(storage_path: PathBuf) -> Self {
        let current_consent = Self::load_from_file(&storage_path);
        Self {
            storage_path,
            current_consent,
            // 신규 인스턴스 생성 시 소거 대기 플래그는 false로 초기화한다.
            pending_deletion: false,
        }
    }

    pub fn check_consent(&self) -> ConsentStatus {
        match &self.current_consent {
            None => ConsentStatus::NotGranted,
            Some(record) => {
                if let Some(expires) = record.expires_at {
                    if Utc::now() > expires {
                        return ConsentStatus::Expired;
                    }
                }
                if record.version != CURRENT_POLICY_VERSION {
                    return ConsentStatus::UpdateRequired;
                }
                ConsentStatus::Valid
            }
        }
    }

    pub fn current_consent(&self) -> Option<&ConsentRecord> {
        self.current_consent.as_ref()
    }

    pub fn grant_consent(
        &mut self,
        permissions: ConsentPermissions,
        data_retention_days: u32,
    ) -> Result<(), CoreError> {
        let record = ConsentRecord {
            consent_id: uuid::Uuid::new_v4().to_string(),
            version: CURRENT_POLICY_VERSION.to_string(),
            granted_at: Utc::now(),
            expires_at: None,
            revoked_at: None,
            data_deletion_requested: false,
            permissions,
            data_retention_days,
        };

        self.save_to_file(&record)?;
        self.current_consent = Some(record);
        Ok(())
    }

    /// Revokes user consent (GDPR Article 7 §3).
    ///
    /// Sets `data_deletion_requested = true` and `revoked_at` on the record,
    /// then atomically writes the updated record to disk (write to .tmp, rename)
    /// so that a crash between write and rename never leaves a partially-written
    /// or deleted file.  `load_from_file()` treats `data_deletion_requested =
    /// true` as revoked, so the file's presence does not re-activate consent.
    pub fn revoke_consent(&mut self) -> Result<(), CoreError> {
        if let Some(record) = self.current_consent.as_mut() {
            record.revoked_at = Some(Utc::now());
            record.data_deletion_requested = true;
        }
        // Clone to release the mutable borrow before writing.
        if let Some(record) = self.current_consent.clone() {
            // Atomic write: serialize to a .tmp file, then rename into place.
            // This eliminates the TOCTOU window that existed when we saved and
            // then deleted the file in two separate syscalls.
            let tmp_path = self.consent_file_path().with_extension("tmp");
            if let Some(parent) = tmp_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let json = serde_json::to_string_pretty(&record)?;
            std::fs::write(&tmp_path, &json)?;
            std::fs::rename(&tmp_path, self.consent_file_path())?;
        }
        self.current_consent = None;
        // current_consent를 None으로 설정한 뒤에도 소거 요청 신호가 소실되지
        // 않도록 in-memory 플래그를 true로 유지한다 (GDPR Article 17).
        self.pending_deletion = true;
        Ok(())
    }

    /// Returns true when consent was previously revoked and local data is
    /// pending erasure (GDPR Article 17).  Callers should purge stored events,
    /// frames, and metrics before the next server sync when this returns true.
    ///
    /// `pending_deletion` 플래그는 `revoke_consent()` 이후 `current_consent`가
    /// None으로 바뀌더라도 true를 유지한다. 데이터 소거 완료 후에는
    /// `clear_pending_deletion()`을 호출해 플래그를 초기화해야 한다.
    pub fn has_pending_deletion(&self) -> bool {
        // in-memory 플래그 우선 확인 — revoke 이후 current_consent가 None이어도
        // 소거 신호가 보존된다.
        self.pending_deletion
            || self
                .current_consent
                .as_ref()
                .map(|r| r.data_deletion_requested)
                .unwrap_or(false)
    }

    /// 데이터 소거 완료 후 호출한다. GDPR Article 17 소거 신호를 초기화한다.
    ///
    /// 이 메서드는 실제 데이터 소거가 완료된 직후에만 호출해야 한다.
    /// 소거 전에 호출하면 삭제 요청이 누락된다.
    pub fn clear_pending_deletion(&mut self) {
        self.pending_deletion = false;
    }

    pub fn is_permitted(&self, check: impl Fn(&ConsentPermissions) -> bool) -> bool {
        self.current_consent
            .as_ref()
            .map(|r| check(&r.permissions))
            .unwrap_or(false)
    }

    /// Returns the canonical path of the consent file (same as `storage_path`).
    fn consent_file_path(&self) -> PathBuf {
        self.storage_path.clone()
    }

    fn load_from_file(path: &PathBuf) -> Option<ConsentRecord> {
        let data = std::fs::read_to_string(path).ok()?;
        let record: ConsentRecord = serde_json::from_str(&data).ok()?;
        // データ削除要求が設定されている場合は同意なしとして扱う。
        // Treat a revoked-but-not-yet-erased record as absent so that a
        // process restart after revoke_consent() does not re-activate consent.
        if record.data_deletion_requested {
            return None;
        }
        Some(record)
    }

    fn save_to_file(&self, record: &ConsentRecord) -> Result<(), CoreError> {
        if let Some(parent) = self.storage_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(record)?;
        std::fs::write(&self.storage_path, json)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consent_permissions_default_all_false() {
        let perms = ConsentPermissions::default();
        assert!(!perms.screen_capture);
        assert!(!perms.telemetry);
        assert!(!perms.clipboard_monitoring);
    }

    #[test]
    fn consent_record_serde_roundtrip() {
        let record = ConsentRecord {
            consent_id: "test-001".to_string(),
            version: CURRENT_POLICY_VERSION.to_string(),
            granted_at: Utc::now(),
            expires_at: None,
            revoked_at: None,
            data_deletion_requested: false,
            permissions: ConsentPermissions::default(),
            data_retention_days: 30,
        };

        let json = serde_json::to_string(&record).unwrap();
        let deserialized: ConsentRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.consent_id, "test-001");
        assert_eq!(deserialized.data_retention_days, 30);
        assert!(deserialized.revoked_at.is_none());
        assert!(!deserialized.data_deletion_requested);
    }

    #[test]
    fn consent_record_serde_legacy_compat() {
        // Records written before revoked_at / data_deletion_requested were added
        // must still deserialize correctly (both fields have #[serde(default)]).
        let legacy_json = r#"{
            "consent_id": "legacy-001",
            "version": "1.0.0",
            "granted_at": "2025-01-01T00:00:00Z",
            "expires_at": null,
            "permissions": {},
            "data_retention_days": 30
        }"#;
        let record: ConsentRecord = serde_json::from_str(legacy_json).unwrap();
        assert_eq!(record.consent_id, "legacy-001");
        assert!(record.revoked_at.is_none());
        assert!(!record.data_deletion_requested);
    }

    #[test]
    fn consent_status_not_granted_when_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let manager = ConsentManager::new(path);
        assert_eq!(manager.check_consent(), ConsentStatus::NotGranted);
    }

    #[test]
    fn consent_grant_and_check() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let mut manager = ConsentManager::new(path);

        let perms = ConsentPermissions {
            screen_capture: true,
            ..Default::default()
        };
        manager.grant_consent(perms, 30).unwrap();

        assert_eq!(manager.check_consent(), ConsentStatus::Valid);
        assert!(manager.is_permitted(|p| p.screen_capture));
        assert!(!manager.is_permitted(|p| p.clipboard_monitoring));
    }

    #[test]
    fn consent_revoke() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let mut manager = ConsentManager::new(path);

        let perms = ConsentPermissions::default();
        manager.grant_consent(perms, 30).unwrap();
        assert_eq!(manager.check_consent(), ConsentStatus::Valid);

        manager.revoke_consent().unwrap();
        assert_eq!(manager.check_consent(), ConsentStatus::NotGranted);
    }

    #[test]
    fn consent_expired() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let mut manager = ConsentManager::new(path);

        let record = ConsentRecord {
            consent_id: "expired-001".to_string(),
            version: CURRENT_POLICY_VERSION.to_string(),
            granted_at: Utc::now() - chrono::Duration::days(365),
            expires_at: Some(Utc::now() - chrono::Duration::days(1)),
            revoked_at: None,
            data_deletion_requested: false,
            permissions: ConsentPermissions::default(),
            data_retention_days: 30,
        };
        manager.current_consent = Some(record);
        assert_eq!(manager.check_consent(), ConsentStatus::Expired);
    }

    #[test]
    fn consent_update_required_on_version_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let mut manager = ConsentManager::new(path);

        let record = ConsentRecord {
            consent_id: "old-001".to_string(),
            version: "0.9.0".to_string(), // previous version
            granted_at: Utc::now(),
            expires_at: None,
            revoked_at: None,
            data_deletion_requested: false,
            permissions: ConsentPermissions::default(),
            data_retention_days: 30,
        };
        manager.current_consent = Some(record);
        assert_eq!(manager.check_consent(), ConsentStatus::UpdateRequired);
    }

    #[test]
    fn has_pending_deletion_false_before_revoke() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let mut manager = ConsentManager::new(path);
        manager
            .grant_consent(ConsentPermissions::default(), 30)
            .unwrap();
        assert!(!manager.has_pending_deletion());
    }

    #[test]
    fn consent_revoke_records_audit_trail() {
        // 동의 철회 후 has_pending_deletion()은 true를 반환해야 한다
        // (GDPR Article 17 소거 신호가 current_consent = None 이후에도 보존되는지 검증).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let mut manager = ConsentManager::new(path);
        manager
            .grant_consent(ConsentPermissions::default(), 30)
            .unwrap();
        assert_eq!(manager.check_consent(), ConsentStatus::Valid);

        manager.revoke_consent().unwrap();
        // 철회 후: 활성 동의 없음
        assert_eq!(manager.check_consent(), ConsentStatus::NotGranted);
        // pending_deletion 플래그는 revoke 이후 true를 유지해야 한다
        assert!(manager.has_pending_deletion());
    }

    #[test]
    fn has_pending_deletion_true_after_revoke() {
        // revoke_consent() → has_pending_deletion() == true →
        // clear_pending_deletion() → has_pending_deletion() == false 전체 라이프사이클 검증.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let mut manager = ConsentManager::new(path);

        // 동의 부여
        manager
            .grant_consent(ConsentPermissions::default(), 30)
            .unwrap();
        assert!(
            !manager.has_pending_deletion(),
            "동의 부여 직후에는 소거 대기가 없어야 한다"
        );

        // 동의 철회
        manager.revoke_consent().unwrap();
        assert!(
            manager.has_pending_deletion(),
            "revoke_consent() 이후 has_pending_deletion()은 true이어야 한다 (GDPR Article 17)"
        );

        // 소거 완료 후 플래그 초기화
        manager.clear_pending_deletion();
        assert!(
            !manager.has_pending_deletion(),
            "clear_pending_deletion() 이후 has_pending_deletion()은 false이어야 한다"
        );
    }

    #[test]
    fn consent_permissions_cross_device_sync_default_false() {
        let perms = ConsentPermissions::default();
        assert!(
            !perms.cross_device_sync,
            "cross_device_sync must default to false (GDPR Article 6)"
        );
    }

    #[test]
    fn consent_cross_device_sync_permission_check() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let mut manager = ConsentManager::new(path);

        // Without cross_device_sync
        let perms = ConsentPermissions::default();
        manager.grant_consent(perms, 30).unwrap();
        assert!(!manager.is_permitted(|p| p.cross_device_sync));

        // With cross_device_sync
        let perms_with_sync = ConsentPermissions {
            cross_device_sync: true,
            ..Default::default()
        };
        manager.grant_consent(perms_with_sync, 30).unwrap();
        assert!(manager.is_permitted(|p| p.cross_device_sync));
    }

    #[test]
    fn consent_without_full_text_extraction_deserializes() {
        let json = r#"{"screen_capture":true,"activity_pattern_learning":true}"#;
        let perms: ConsentPermissions = serde_json::from_str(json).unwrap();
        assert!(!perms.full_text_extraction);
        assert!(perms.activity_pattern_learning);
    }

    #[test]
    fn revoke_persists_file_and_new_manager_sees_not_granted() {
        // After revoke_consent(), the consent file must still exist on disk (no
        // TOCTOU delete), but a freshly constructed ConsentManager pointing at
        // the same path must report NotGranted because data_deletion_requested
        // is set in the persisted record.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let mut manager = ConsentManager::new(path.clone());
        manager
            .grant_consent(ConsentPermissions::default(), 30)
            .unwrap();
        assert_eq!(manager.check_consent(), ConsentStatus::Valid);
        assert!(path.exists(), "consent file must exist after grant");

        manager.revoke_consent().unwrap();
        assert!(
            path.exists(),
            "revoke must keep the file (atomic flag, not delete)"
        );

        // Verify the on-disk record has data_deletion_requested = true.
        let raw = std::fs::read_to_string(&path).unwrap();
        let record: ConsentRecord = serde_json::from_str(&raw).unwrap();
        assert!(
            record.data_deletion_requested,
            "file must have deletion flag set"
        );

        // A new manager reading the same file must treat it as revoked.
        let new_manager = ConsentManager::new(path.clone());
        assert_eq!(
            new_manager.check_consent(),
            ConsentStatus::NotGranted,
            "new manager must see NotGranted for a revoked record"
        );
    }

    #[test]
    fn consent_permissions_legacy_json_without_cross_device_sync() {
        // Records written before cross_device_sync was added must deserialize.
        let legacy_json = r#"{
            "screen_capture": true,
            "ocr_processing": false,
            "telemetry": true,
            "process_monitoring": true,
            "input_activity": false,
            "window_title_collection": false,
            "app_usage_analytics": false,
            "clipboard_monitoring": false,
            "file_access_monitoring": false,
            "activity_pattern_learning": false
        }"#;
        let perms: ConsentPermissions = serde_json::from_str(legacy_json).unwrap();
        assert!(perms.screen_capture);
        assert!(
            !perms.cross_device_sync,
            "missing field must default to false"
        );
    }
}
