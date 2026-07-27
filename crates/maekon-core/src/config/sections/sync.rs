// Cross-device sync configuration (Phase 3 — P3).
use serde::{Deserialize, Serialize};

/// Minimum allowed sync interval. Callers should use
/// `SyncConfig::validated_interval_secs()` to ensure the value is at least
/// this floor (prevents excessive network/battery drain).
pub const MIN_SYNC_INTERVAL_SECS: u64 = 30;

/// OS keychain service + account holding the cross-device sync passphrase
/// (#8056 P2-2). The passphrase lives in the OS keychain — NOT in config — so a
/// GUI app launched from Finder / the Start menu (which does not inherit a
/// shell's `MAEKON_SYNC_PASSPHRASE` env) can still activate sync. Shared between
/// the read side (`agent_runtime::sync_setup`) and the Settings → Sync write
/// route so the two never drift on the entry name.
pub const SYNC_PASSPHRASE_KEYCHAIN_SERVICE: &str = "maekon";
/// See [`SYNC_PASSPHRASE_KEYCHAIN_SERVICE`].
pub const SYNC_PASSPHRASE_KEYCHAIN_ACCOUNT: &str = "sync_passphrase";

/// Minimum length for the sync passphrase. Argon2id derives BOTH the AES-256-GCM
/// payload key and the LAN HMAC challenge-response key from it, so a trivially
/// short value yields a weak key. This is a length floor, not an entropy
/// guarantee. Shared by the runtime setup gate and the write route.
pub const MIN_SYNC_PASSPHRASE_LEN: usize = 12;

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

impl std::fmt::Display for SyncTransportKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Remote => f.write_str("remote"),
            Self::File => f.write_str("file"),
            Self::Lan => f.write_str("lan"),
        }
    }
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

impl std::fmt::Display for RemoteSyncAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BearerToken => f.write_str("bearer_token"),
            Self::ApiKey => f.write_str("api_key"),
        }
    }
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
    /// Default: false (only dominant_category, duration, app_breakdown sync).
    ///
    /// ⚠️ PRIVACY: `content_activities_json` is screen-content-derived personal data;
    /// a user-facing toggle / consent disclosure SHOULD say so. The `cross_device_sync`
    /// consent gate still applies on top of this flag. (`llm_summary` has its own
    /// `include_llm_summary` gate below.)
    #[serde(default)]
    pub include_content_activities: bool,

    /// Include the LLM `llm_summary` narrative in synced segments.
    /// Default: false (segments sync without the LLM summary).
    ///
    /// ⚠️ PRIVACY: `llm_summary` is an LLM narrative of the user's screen activity —
    /// e.g. "reviewed the Q2 headcount spreadsheet" — i.e. screen-derived personal data.
    /// Before #5174 it was in the baseline payload and propagated unconditionally; it is
    /// now gated here (parity with `include_content_activities` / `include_embedding_text`),
    /// default false so the LLM summary is NOT sent to peers unless explicitly opted in.
    /// Any user-facing toggle MUST disclose that enabling it sends an LLM narrative of
    /// screen activity to other devices. The `cross_device_sync` consent gate still applies.
    /// The flag is FORWARD-ONLY: disabling it stops future sends but does NOT retract an
    /// `llm_summary` already propagated to a peer — only a GDPR Art.17 erasure (#5174)
    /// hard-deletes those. (Note: a SegmentSummary embedding's `original_text` carries the
    /// same narrative and syncs under `include_embedding_text` — see that flag.)
    #[serde(default)]
    pub include_llm_summary: bool,

    /// Include `original_text` in synced embedding vectors.
    /// Default: false (only vector blobs sync).
    ///
    /// ⚠️ PRIVACY: `original_text` is **user-content-derived personal data** — the raw
    /// text an embedding was built from (document/screen content, window titles, typed
    /// text). Enabling this propagates that content to every synced peer. Since F0/#5186
    /// began HLC-stamping embedding writes these rows now propagate continuously (not just
    /// once), so this flag's exposure is material: any user-facing toggle MUST disclose
    /// that enabling it sends derived screen/document content to other devices. The
    /// `cross_device_sync` consent gate still applies on top of this flag.
    ///
    /// #5210 cross-gate composition: a `SEGMENT_SUMMARY` embedding's text/vector ARE the
    /// `llm_summary` narrative, so when `include_llm_summary` is OFF those embedding rows
    /// are excluded from sync entirely even if THIS flag is on — enabling embedding-text
    /// sync is not a backdoor that re-exposes the gated LLM screen-activity narrative.
    #[serde(default)]
    pub include_embedding_text: bool,

    /// Human-readable name for this device (e.g., "Work MacBook").
    /// Shown to the user on peer devices. Defaults to OS hostname.
    #[serde(default = "default_device_name")]
    pub device_name: String,

    /// Path to the shared sync folder (Dropbox, iCloud, NAS mount, etc.).
    /// Required when `transport == SyncTransportKind::File`.
    /// Example: "~/Dropbox/maekon-sync" or "/Volumes/NAS/sync".
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
            include_llm_summary: false,
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
            sync_folder: Some("/Users/test/Dropbox/maekon-sync".to_string()),
            passphrase_hash: Some("$argon2id$v=19$m=65536,t=3,p=4$...".to_string()),
            ..SyncConfig::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: SyncConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(
            parsed.sync_folder.as_deref(),
            Some("/Users/test/Dropbox/maekon-sync")
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

    #[test]
    fn remote_sync_auth_display_matches_serde_token() {
        assert_eq!(RemoteSyncAuth::BearerToken.to_string(), "bearer_token");
        assert_eq!(RemoteSyncAuth::ApiKey.to_string(), "api_key");
    }
}
