use base64::Engine;
use maekon_core::config::{AnalysisConfig, ExternalDataPolicy, PrivacyConfig};
use std::time::Duration;

// #7731 (ctd-W2 B4, 2026-07-03): this file used to be a grab-bag holding the
// `SchedulerStorage` trait + its ~160-line forwarding impl, `PlatformEgressPolicy`,
// and `SchedulerConfig` all together. `SchedulerStorage` moved to
// `maekon_core::ports::scheduler_storage` (implemented directly on
// `SqliteStorage` in `maekon_storage::sqlite::scheduler_storage_impl`), and
// `PlatformEgressPolicy` moved to `scheduler::egress_policy`. This file now
// holds only `SchedulerConfig` and the small set of scheduler-wide tuning
// constants that don't belong to either of those.

pub(super) fn base64_decode(input: &str) -> Result<Vec<u8>, String> {
    base64::engine::general_purpose::STANDARD
        .decode(input)
        .map_err(|e| e.to_string())
}

/// Retention: raw system metrics are kept for 24 hours.
pub(super) const RAW_METRICS_RETENTION_HOURS: i64 = 24;
/// Retention: hourly metric rollups are kept for 30 days (V3 migration contract).
pub(super) const HOURLY_METRICS_RETENTION_DAYS: i64 = 30;
/// Retention: process snapshots are kept for 7 days.
pub(super) const PROCESS_SNAPSHOT_RETENTION_DAYS: i64 = 7;
/// Retention: idle period records are kept for 30 days.
pub(super) const IDLE_PERIOD_RETENTION_DAYS: i64 = 30;

/// OAuth token refresh check interval (seconds).
#[cfg(feature = "analysis")]
pub(super) const OAUTH_REFRESH_INTERVAL_SECS: u64 = 120;

/// Coaching evaluation interval — 30 seconds.
pub(super) const COACHING_INTERVAL_SECS: u64 = 30;

/// Adapter health check interval — 5 seconds.
pub(super) const HEALTH_CHECK_INTERVAL_SECS: u64 = 5;

/// SQLite WAL checkpoint + FTS merge interval — 5 minutes.
pub(super) const SQLITE_MAINTENANCE_INTERVAL_MINS: i64 = 5;

/// Regime state periodic checkpoint interval — 30 minutes (#5810).
///
/// The shutdown path (main.rs RunEvent::Exit) remains the authoritative
/// save; this periodic checkpoint is a crash-durability supplement only.
/// 30 minutes is long enough to avoid contention with the 5-minute SQLite
/// maintenance window, yet short enough to cap session-loss on unclean exit.
pub(super) const REGIME_CHECKPOINT_INTERVAL_MINS: i64 = 30;

/// Freelist threshold (%) above which VACUUM is triggered.
pub(super) const VACUUM_FREELIST_THRESHOLD_PERCENT: u64 = 20;

/// Number of FTS5 b-tree pages to merge per maintenance tick.
pub(super) const FTS_MERGE_PAGES: u32 = 64;

pub struct SchedulerConfig {
    pub poll_interval: Duration,
    pub metrics_interval: Duration,
    pub process_interval: Duration,
    pub detailed_process_interval: Duration,
    pub input_activity_interval: Duration,
    pub sync_interval: Duration,
    pub heartbeat_interval: Duration,
    pub aggregation_interval: Duration,
    pub session_id: String,
    pub external_data_policy: ExternalDataPolicy,
    pub privacy_config: PrivacyConfig,
    pub idle_threshold_secs: u64,
    pub upload_enabled: bool,
    pub analysis_config: AnalysisConfig,
    /// Interval for cross-device sync loop (P3 Phase 3a-2).
    pub cross_device_sync_interval: Duration,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(1),
            metrics_interval: Duration::from_secs(5),
            process_interval: Duration::from_secs(10),
            detailed_process_interval: Duration::from_secs(30), // 30 s
            input_activity_interval: Duration::from_secs(30),   // 30 s
            sync_interval: Duration::from_secs(10),
            heartbeat_interval: Duration::from_secs(30),
            aggregation_interval: Duration::from_secs(3600), // 1 hour
            session_id: String::new(),                       // set by caller
            external_data_policy: ExternalDataPolicy::default(),
            privacy_config: PrivacyConfig::default(),
            idle_threshold_secs: 300, // 5 min
            upload_enabled: false,
            analysis_config: AnalysisConfig::default(),
            cross_device_sync_interval: Duration::from_secs(300), // 5 min default
        }
    }
}

impl SchedulerConfig {
    /// #6442 (F10): true when the egress policy/level pairing is incoherent — AllowFiltered
    /// ("egress, but filter PII") with PII filtering turned Off. The egress paths floor to
    /// Basic regardless (`ExternalDataPolicy::effective_egress_pii_level`), but the caller
    /// surfaces this as a loud, one-time config-validation error at load so the user gets
    /// explicit feedback instead of the old silent per-call floor upgrade (#5992).
    pub fn has_incoherent_egress_privacy(&self) -> bool {
        self.external_data_policy == ExternalDataPolicy::AllowFiltered
            && self.privacy_config.pii_filter_level == maekon_core::config::PiiFilterLevel::Off
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maekon_core::config::PiiFilterLevel;

    #[test]
    fn detects_incoherent_egress_privacy_pairing() {
        // #6442 F10: AllowFiltered + Off is the incoherent "filter PII but disable the
        // filter" pairing the loud load-time validation flags.
        let incoherent = SchedulerConfig {
            external_data_policy: ExternalDataPolicy::AllowFiltered,
            privacy_config: PrivacyConfig {
                pii_filter_level: PiiFilterLevel::Off,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(incoherent.has_incoherent_egress_privacy());

        // AllowFiltered with a real filter level is coherent.
        let coherent = SchedulerConfig {
            external_data_policy: ExternalDataPolicy::AllowFiltered,
            privacy_config: PrivacyConfig {
                pii_filter_level: PiiFilterLevel::Basic,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(!coherent.has_incoherent_egress_privacy());

        // A redacting policy never relies on the configured level, so Off is not
        // incoherent there.
        let strict = SchedulerConfig {
            external_data_policy: ExternalDataPolicy::PiiFilterStrict,
            privacy_config: PrivacyConfig {
                pii_filter_level: PiiFilterLevel::Off,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(!strict.has_incoherent_egress_privacy());
    }
}
