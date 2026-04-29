// Cross-device sync configuration (Phase 3 — P3).
use serde::{Deserialize, Serialize};

/// Minimum allowed sync interval. Callers should use
/// `SyncConfig::validated_interval_secs()` to ensure the value is at least
/// this floor (prevents excessive network/battery drain).
pub const MIN_SYNC_INTERVAL_SECS: u64 = 30;

/// Sync transport selection.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncTransportKind {
    /// Push/pull via REST/gRPC to a remote sync endpoint.
    Remote,
    /// Read/write encrypted JSON to a shared folder (Dropbox, iCloud, NAS).
    #[default]
    File,
    /// mDNS discovery + direct TCP between devices on the same LAN (Phase 3b).
    Lan,
}

/// Authentication mode for remote sync transport.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteSyncAuth {
    /// Bearer token authentication (e.g., JWT from connected server login).
    #[default]
    BearerToken,
    /// Static API key (self-hosted scenarios).
    ApiKey,
}

/// Cross-device sync configuration.
///
/// Controls whether activity data is synchronized between devices
/// owned by the same user. Default: disabled. Both `enabled` AND
/// `ConsentPermissions::cross_device_sync` must be true for any
/// data to leave the device.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncConfig {
    /// Master switch. When false, all sync operations are disabled.
    #[serde(default)]
    pub enabled: bool,

    /// Selected transport mechanism.
    #[serde(default)]
    pub transport: SyncTransportKind,

    /// Interval between periodic sync cycles (seconds). Default: 300 (5 min).
    #[serde(default = "default_sync_interval_secs")]
    pub interval_secs: u64,

    /// Include raw `content_activities_json` in synced segments.
    /// Default: false (only dominant_category, duration, app_breakdown,
    /// llm_summary are synced).
    #[serde(default)]
    pub include_content_activities: bool,

    /// Include `original_text` in synced embedding vectors.
    /// Default: false (only vector blobs sync).
    #[serde(default)]
    pub include_embedding_text: bool,

    /// Human-readable name for this device (e.g., "Work MacBook").
    /// Shown to the user on peer devices. Defaults to OS hostname.
    #[serde(default = "default_device_name")]
    pub device_name: String,

    /// Path to the shared sync folder (Dropbox, iCloud, NAS mount, etc.).
    /// Required when `transport == SyncTransportKind::File`.
    /// Example: "~/Dropbox/oneshim-sync" or "/Volumes/NAS/sync".
    #[serde(default)]
    pub sync_folder: Option<String>,

    /// Argon2id hash of the user-chosen sync passphrase.
    /// Stored only for passphrase verification on new device setup.
    /// The actual AES-256-GCM key is derived at runtime via Argon2id KDF.
    /// Never contains the raw passphrase.
    #[serde(default)]
    pub passphrase_hash: Option<String>,

    /// Remote sync endpoint URL. Required when transport == Remote.
    /// Example: `<https://sync.example.com/api/v1>`
    #[serde(default)]
    pub remote_endpoint: Option<String>,

    /// Authentication mode for the remote endpoint.
    #[serde(default)]
    pub remote_auth: RemoteSyncAuth,

    /// Port for the LAN sync HTTPS server. 0 = ephemeral (auto-assigned).
    /// Default: 0.
    #[serde(default)]
    pub lan_port: u16,

    /// Whether to advertise this device via mDNS for LAN discovery.
    /// Default: true (when transport == Lan).
    #[serde(default = "default_true")]
    pub lan_advertise: bool,

    /// Whether to compress changeset payloads before encryption.
    /// Reduces network bandwidth at the cost of a small CPU overhead.
    /// Default: true.
    #[serde(default = "default_true")]
    pub compression_enabled: bool,
}

fn default_sync_interval_secs() -> u64 {
    300
}

fn default_true() -> bool {
    true
}

fn default_device_name() -> String {
    gethostname::gethostname().to_string_lossy().into_owned()
}

impl SyncConfig {
    /// Return `interval_secs` clamped to at least [`MIN_SYNC_INTERVAL_SECS`].
    pub fn validated_interval_secs(&self) -> u64 {
        self.interval_secs.max(MIN_SYNC_INTERVAL_SECS)
    }
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            transport: SyncTransportKind::default(),
            interval_secs: default_sync_interval_secs(),
            include_content_activities: false,
            include_embedding_text: false,
            device_name: default_device_name(),
            sync_folder: None,
            passphrase_hash: None,
            remote_endpoint: None,
            remote_auth: RemoteSyncAuth::default(),
            lan_port: 0,
            lan_advertise: true,
            compression_enabled: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_config_default_is_disabled() {
        let config = SyncConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.transport, SyncTransportKind::File);
        assert_eq!(config.interval_secs, 300);
        assert!(!config.include_content_activities);
        assert!(!config.include_embedding_text);
        assert!(!config.device_name.is_empty());
    }

    #[test]
    fn sync_config_folder_and_passphrase_default_none() {
        let config = SyncConfig::default();
        assert!(config.sync_folder.is_none());
        assert!(config.passphrase_hash.is_none());
    }

    #[test]
    fn sync_config_with_folder_serde_roundtrip() {
        let config = SyncConfig {
            enabled: true,
            sync_folder: Some("/Users/test/Dropbox/oneshim-sync".to_string()),
            passphrase_hash: Some("$argon2id$v=19$m=65536,t=3,p=4$...".to_string()),
            ..SyncConfig::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: SyncConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(
            parsed.sync_folder.as_deref(),
            Some("/Users/test/Dropbox/oneshim-sync")
        );
        assert!(parsed.passphrase_hash.is_some());
    }

    #[test]
    fn sync_config_serde_roundtrip() {
        let config = SyncConfig {
            enabled: true,
            transport: SyncTransportKind::Remote,
            interval_secs: 600,
            include_content_activities: true,
            include_embedding_text: false,
            device_name: "Test Machine".to_string(),
            ..SyncConfig::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: SyncConfig = serde_json::from_str(&json).unwrap();
        assert!(parsed.enabled);
        assert_eq!(parsed.transport, SyncTransportKind::Remote);
        assert_eq!(parsed.interval_secs, 600);
        assert!(parsed.include_content_activities);
        assert_eq!(parsed.device_name, "Test Machine");
    }

    #[test]
    fn sync_config_empty_json_uses_defaults() {
        let parsed: SyncConfig = serde_json::from_str("{}").unwrap();
        assert!(!parsed.enabled);
        assert_eq!(parsed.transport, SyncTransportKind::File);
        assert_eq!(parsed.interval_secs, 300);
    }

    #[test]
    fn sync_transport_kind_serde_snake_case() {
        let json = serde_json::to_string(&SyncTransportKind::Remote).unwrap();
        assert_eq!(json, "\"remote\"");
        let json = serde_json::to_string(&SyncTransportKind::Lan).unwrap();
        assert_eq!(json, "\"lan\"");
    }

    #[test]
    fn interval_secs_clamped_to_minimum() {
        let config = SyncConfig {
            interval_secs: 5, // well below MIN_SYNC_INTERVAL_SECS
            ..SyncConfig::default()
        };
        assert_eq!(config.validated_interval_secs(), MIN_SYNC_INTERVAL_SECS);

        // At the boundary
        let config2 = SyncConfig {
            interval_secs: MIN_SYNC_INTERVAL_SECS,
            ..SyncConfig::default()
        };
        assert_eq!(config2.validated_interval_secs(), MIN_SYNC_INTERVAL_SECS);

        // Above the minimum — unchanged
        let config3 = SyncConfig {
            interval_secs: 600,
            ..SyncConfig::default()
        };
        assert_eq!(config3.validated_interval_secs(), 600);
    }

    #[test]
    fn remote_auth_serde_snake_case() {
        let json = serde_json::to_string(&RemoteSyncAuth::BearerToken).unwrap();
        assert_eq!(json, "\"bearer_token\"");
        let json = serde_json::to_string(&RemoteSyncAuth::ApiKey).unwrap();
        assert_eq!(json, "\"api_key\"");
    }

    #[test]
    fn sync_config_remote_fields_default() {
        let config = SyncConfig::default();
        assert!(config.remote_endpoint.is_none());
        assert_eq!(config.remote_auth, RemoteSyncAuth::BearerToken);
        assert_eq!(config.lan_port, 0);
        assert!(config.lan_advertise);
    }

    #[test]
    fn sync_config_remote_serde_roundtrip() {
        let config = SyncConfig {
            enabled: true,
            transport: SyncTransportKind::Remote,
            remote_endpoint: Some("https://sync.example.com/api/v1".to_string()),
            remote_auth: RemoteSyncAuth::ApiKey,
            ..SyncConfig::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: SyncConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(
            parsed.remote_endpoint.as_deref(),
            Some("https://sync.example.com/api/v1")
        );
        assert_eq!(parsed.remote_auth, RemoteSyncAuth::ApiKey);
    }

    #[test]
    fn app_config_with_sync_section_deserializes() {
        // Existing configs without a "sync" key must still parse
        // (the #[serde(default)] on AppConfig::sync handles this).
        let minimal = r#"{ "server": { "base_url": "http://localhost:8000", "request_timeout_ms": 5000, "sse_max_retry_secs": 30 }, "monitor": { "poll_interval_ms": 1000, "sync_interval_ms": 10000, "heartbeat_interval_ms": 60000, "idle_threshold_secs": 300, "process_interval_secs": 10, "process_monitoring": true, "input_activity": true, "upload_enabled": false }, "storage": { "retention_days": 30, "max_storage_mb": 500 }, "vision": { "capture_enabled": false, "capture_throttle_ms": 5000, "thumbnail_width": 480, "thumbnail_height": 270, "ocr_enabled": false, "privacy_mode": false } }"#;
        let config: crate::config::AppConfig = serde_json::from_str(minimal).unwrap();
        assert!(!config.sync.enabled);
    }
}
