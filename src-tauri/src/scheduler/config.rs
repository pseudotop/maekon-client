use base64::Engine;
use chrono::{DateTime, Utc};
use maekon_core::config::{AnalysisConfig, ExternalDataPolicy, PrivacyConfig};
use maekon_core::error::CoreError;
use maekon_core::models::context::WindowBounds;
use maekon_core::models::event::Event;
use maekon_core::models::frame::FrameMetadata;
use maekon_core::models::tiered_memory::SegmentSummary;
use maekon_core::ports::consent_manager::ConsentManagerPort;
use maekon_core::ports::storage::MetricsStorage;
use maekon_storage::sqlite::SqliteStorage;
use maekon_vision::privacy::{sanitize_title_with_level, should_exclude};
use std::sync::Arc;
use std::time::Duration;

pub trait SchedulerStorage: MetricsStorage + Send + Sync {
    fn save_frame_metadata_with_bounds(
        &self,
        metadata: &FrameMetadata,
        file_path: Option<&str>,
        ocr_text: Option<&str>,
        bounds: Option<&WindowBounds>,
    ) -> Result<i64, CoreError>;

    /// Check whether server-sourced suggestions exist within the given lookback
    /// window (in seconds). Used by the analysis loop to suppress local LLM
    /// analysis when the server is actively providing suggestions.
    fn has_recent_server_suggestions(&self, lookback_secs: u64) -> Result<bool, CoreError>;

    /// List recent weekly digests, newest first.
    fn list_weekly_digests(
        &self,
        limit: usize,
    ) -> Result<Vec<maekon_core::models::weekly_digest::WeeklyDigest>, CoreError>;

    /// Save a weekly digest. Upserts by week_start.
    fn save_weekly_digest(
        &self,
        digest: &maekon_core::models::weekly_digest::WeeklyDigest,
    ) -> Result<(), CoreError>;

    /// List closed segments whose time range falls within [from, to].
    fn list_segments_between(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<SegmentSummary>, CoreError>;

    /// Delete activity segments older than `max_days`. Returns the count of deleted rows.
    fn enforce_segment_retention(&self, max_days: u32) -> Result<usize, CoreError>;

    /// Delete weekly digests older than `max_weeks`. Returns the count of deleted rows.
    fn enforce_digest_retention(&self, max_weeks: u32) -> Result<usize, CoreError>;

    /// Get a cached daily digest by date (YYYY-MM-DD).
    fn get_daily_digest(
        &self,
        date: &str,
    ) -> Result<Option<maekon_core::models::daily_digest::DailyDigest>, CoreError>;

    /// Save a daily digest. Upserts by date.
    fn save_daily_digest(
        &self,
        digest: &maekon_core::models::daily_digest::DailyDigest,
    ) -> Result<(), CoreError>;

    /// Get activity segment summary records for a given date (YYYY-MM-DD).
    fn get_segments_for_date(
        &self,
        date: &str,
    ) -> Result<Vec<maekon_core::models::storage_records::SegmentSummaryRecord>, CoreError>;

    /// Save a GUI interaction event (delegates to WebStorage V13 table).
    #[allow(dead_code)] // Wired in capture loop when GUI detection is active
    fn save_gui_interaction(
        &self,
        input: &maekon_core::models::storage_records::NewGuiInteraction<'_>,
    ) -> Result<(), CoreError>;

    /// Enforce retention for all auxiliary tables (work_sessions, interruptions,
    /// gui_interactions, suggestions, local_suggestions, focus_metrics,
    /// daily_digests, regime_overrides). Returns total rows deleted.
    fn enforce_all_retention(&self) -> Result<u64, CoreError>;

    /// GC the GDPR Art.17 erasure tombstone outbox (#5174 S5/R4): hard-delete
    /// `sync_tombstones` older than `max(data_retention_days, 90)` days. Returns
    /// rows deleted. (Trait method so the scheduler can call it through the
    /// `dyn SchedulerStorage` seam — the inherent `SqliteStorage` impl does the work.)
    fn gc_sync_tombstones(&self, data_retention_days: u32) -> Result<usize, CoreError>;

    /// Persist a habit-streak day row (#5669). Called by the coaching loop when
    /// goal minutes flush, giving the HabitTracker widget a local producer —
    /// the `habit_streaks` table previously had no production writer, so the
    /// widget rendered "No data" forever on standalone. Default no-op keeps
    /// non-SQLite test doubles compiling without tracking habits.
    fn upsert_habit_streak(
        &self,
        _regime_label: &str,
        _date: &str,
        _minutes_logged: u32,
        _target_minutes: u32,
        _met: bool,
    ) -> Result<(), CoreError> {
        Ok(())
    }

    // --- SQLite maintenance methods ---

    /// Execute a passive WAL checkpoint. PASSIVE mode is non-blocking and
    /// safe to call while concurrent reads are in progress.
    fn wal_checkpoint_passive(&self) -> Result<(), CoreError>;

    /// Run VACUUM if the freelist occupies more than `threshold_percent` of
    /// the total page count. Returns `true` if VACUUM was actually executed.
    fn maybe_vacuum(&self, threshold_percent: u64) -> Result<bool, CoreError>;

    /// Incrementally merge FTS5 b-tree segments. Call periodically (every
    /// 5-10 minutes) to keep write-amplification low.
    fn fts_merge(&self, pages: u32) -> Result<(), CoreError>;

    /// Run a full FTS5 optimize pass (merges all segments into one). Expensive
    /// but dramatically speeds up subsequent queries. Call once daily.
    fn fts_optimize(&self) -> Result<(), CoreError>;

    /// Run `ANALYZE` to refresh SQLite query planner statistics. Call after
    /// bulk operations (IVF index builds, large batch inserts).
    #[allow(dead_code)] // Called after bulk IVF index builds in maintenance loop
    fn run_analyze(&self) -> Result<(), CoreError>;

    /// egress 한 건을 감사 원장(`egress_ledger`)에 기록한다 (V36, #4803/E20).
    ///
    /// 디바이스를 떠난(`uploaded`) 또는 정책상 차단된(`blocked`) 이벤트를
    /// 규제 준수 증거로 남긴다. 동기 루프(events/monitor)에서 호출하며,
    /// `record_id` UNIQUE 로 재실행 중복을 제거한다.
    fn record_egress(
        &self,
        record: &maekon_core::models::storage_records::EgressLedgerRecord,
    ) -> Result<(), CoreError>;
}

impl SchedulerStorage for SqliteStorage {
    fn save_frame_metadata_with_bounds(
        &self,
        metadata: &FrameMetadata,
        file_path: Option<&str>,
        ocr_text: Option<&str>,
        bounds: Option<&WindowBounds>,
    ) -> Result<i64, CoreError> {
        SqliteStorage::save_frame_metadata_with_bounds(self, metadata, file_path, ocr_text, bounds)
            .map_err(Into::into)
    }

    fn has_recent_server_suggestions(&self, lookback_secs: u64) -> Result<bool, CoreError> {
        SqliteStorage::has_recent_server_suggestions(self, lookback_secs).map_err(Into::into)
    }

    fn list_weekly_digests(
        &self,
        limit: usize,
    ) -> Result<Vec<maekon_core::models::weekly_digest::WeeklyDigest>, CoreError> {
        // ADR-026 PR-6: `DigestStorage` is now async; this sync scheduler trait
        // resolves to the inherent synchronous `SqliteStorage` twin instead.
        SqliteStorage::list_weekly_digests(self, limit)
    }

    fn save_weekly_digest(
        &self,
        digest: &maekon_core::models::weekly_digest::WeeklyDigest,
    ) -> Result<(), CoreError> {
        SqliteStorage::save_weekly_digest(self, digest)
    }

    fn list_segments_between(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<SegmentSummary>, CoreError> {
        SqliteStorage::list_segments_between(self, from, to).map_err(Into::into)
    }

    fn enforce_segment_retention(&self, max_days: u32) -> Result<usize, CoreError> {
        SqliteStorage::enforce_segment_retention(self, max_days).map_err(Into::into)
    }

    fn enforce_digest_retention(&self, max_weeks: u32) -> Result<usize, CoreError> {
        SqliteStorage::enforce_digest_retention(self, max_weeks).map_err(Into::into)
    }

    fn get_daily_digest(
        &self,
        date: &str,
    ) -> Result<Option<maekon_core::models::daily_digest::DailyDigest>, CoreError> {
        SqliteStorage::get_daily_digest(self, date)
    }

    fn save_daily_digest(
        &self,
        digest: &maekon_core::models::daily_digest::DailyDigest,
    ) -> Result<(), CoreError> {
        SqliteStorage::save_daily_digest(self, digest)
    }

    fn get_segments_for_date(
        &self,
        date: &str,
    ) -> Result<Vec<maekon_core::models::storage_records::SegmentSummaryRecord>, CoreError> {
        SqliteStorage::get_segments_for_date(self, date)
    }

    fn save_gui_interaction(
        &self,
        input: &maekon_core::models::storage_records::NewGuiInteraction<'_>,
    ) -> Result<(), CoreError> {
        // ADR-026 PR-7: `GuiInteractionStorage` is now async; this sync scheduler
        // trait resolves to the inherent synchronous `SqliteStorage` twin instead.
        SqliteStorage::save_gui_interaction(self, input).map_err(Into::into)
    }

    fn enforce_all_retention(&self) -> Result<u64, CoreError> {
        SqliteStorage::enforce_all_retention(self).map_err(Into::into)
    }

    fn gc_sync_tombstones(&self, data_retention_days: u32) -> Result<usize, CoreError> {
        SqliteStorage::gc_sync_tombstones(self, data_retention_days).map_err(Into::into)
    }

    fn wal_checkpoint_passive(&self) -> Result<(), CoreError> {
        SqliteStorage::wal_checkpoint_passive(self).map_err(Into::into)
    }

    fn maybe_vacuum(&self, threshold_percent: u64) -> Result<bool, CoreError> {
        SqliteStorage::maybe_vacuum(self, threshold_percent).map_err(Into::into)
    }

    fn fts_merge(&self, pages: u32) -> Result<(), CoreError> {
        SqliteStorage::fts_merge(self, pages).map_err(Into::into)
    }

    fn fts_optimize(&self) -> Result<(), CoreError> {
        SqliteStorage::fts_optimize(self).map_err(Into::into)
    }

    fn run_analyze(&self) -> Result<(), CoreError> {
        SqliteStorage::run_analyze(self).map_err(Into::into)
    }

    fn record_egress(
        &self,
        record: &maekon_core::models::storage_records::EgressLedgerRecord,
    ) -> Result<(), CoreError> {
        SqliteStorage::record_egress(self, record).map_err(Into::into)
    }

    fn upsert_habit_streak(
        &self,
        regime_label: &str,
        date: &str,
        minutes_logged: u32,
        target_minutes: u32,
        met: bool,
    ) -> Result<(), CoreError> {
        // Forwards to the pre-existing inherent sync twin (habit_storage.rs);
        // callers offload via spawn_blocking (write_lock must not run inline
        // on the async monitor loop).
        SqliteStorage::upsert_habit_streak(
            self,
            regime_label,
            date,
            minutes_logged,
            target_minutes,
            met,
        )
        .map_err(Into::into)
    }
}

/// egress 대상(sink 타깃) 문자열 — 배치 업로더는 단일 서버 엔드포인트로 전송한다.
/// `BatchSink` 포트는 타깃 문자열을 노출하지 않으므로 상수로 기록한다(#4803).
pub(super) const EGRESS_DESTINATION_BATCH_UPLOAD: &str = "server.batch_upload";

/// `Event` 의 유형 식별 문자열을 반환한다(egress_ledger.event_type 용).
pub(super) fn egress_event_type(event: &Event) -> &'static str {
    match event {
        Event::Context(_) => "Context",
        Event::Window(_) => "Window",
        Event::User(_) => "User",
        Event::Input(_) => "Input",
        Event::Process(_) => "Process",
        Event::System(_) => "System",
        Event::Clipboard(_) => "Clipboard",
        Event::FileAccess(_) => "FileAccess",
    }
}

/// 이벤트의 직렬화 payload 바이트 크기. 직렬화 실패 시 0 을 반환한다.
pub(super) fn egress_byte_count(event: &Event) -> i64 {
    serde_json::to_vec(event)
        .map(|v| v.len() as i64)
        .unwrap_or(0)
}

/// egress 한 건을 감사 원장에 기록한다(#4803/E20).
///
/// `prepare_event_for_upload` 가 이벤트를 소비하므로, 호출자는 소비 *전* 에
/// 참조로부터 `event_type`/`byte_count` 를 계산해 이 헬퍼에 전달한다.
/// `disposition` 은 `uploaded`(sink enqueue 성공) 또는 `blocked`(정책상 제외)다.
/// `consent_state` 는 egress 시점의 텔레메트리 동의 스냅샷 문자열이다.
/// 기록 실패는 업로드/캡처 흐름을 방해하지 않도록 `warn!` 로만 남긴다.
#[allow(clippy::too_many_arguments)]
pub(super) fn record_event_egress(
    storage: &Arc<dyn SchedulerStorage>,
    event_type: &str,
    byte_count: i64,
    disposition: &str,
    consent_state: &str,
) {
    use maekon_core::models::storage_records::EgressLedgerRecord;
    let record = EgressLedgerRecord {
        record_id: uuid::Uuid::new_v4().to_string(),
        event_type: event_type.to_string(),
        event_id: None,
        byte_count,
        // Telemetry uploads to a single endpoint (server.batch_upload) → 1 recipient.
        recipient_count: 1,
        destination: EGRESS_DESTINATION_BATCH_UPLOAD.to_string(),
        disposition: disposition.to_string(),
        consent_state: consent_state.to_string(),
        occurred_at: Utc::now().to_rfc3339(),
    };
    if let Err(e) = storage.record_egress(&record) {
        tracing::warn!(err.code = %e.code(), "egress ledger 기록 실패: {e}");
    }
}

pub(super) fn base64_decode(input: &str) -> Result<Vec<u8>, String> {
    base64::engine::general_purpose::STANDARD
        .decode(input)
        .map_err(|e| e.to_string())
}

pub(super) const REDACTED_WINDOW_TITLE: &str = "[REDACTED_WINDOW_TITLE]";

/// Retention: raw system metrics are kept for 24 hours.
pub(super) const RAW_METRICS_RETENTION_HOURS: i64 = 24;
/// Retention: process snapshots are kept for 7 days.
pub(super) const PROCESS_SNAPSHOT_RETENTION_DAYS: i64 = 7;
/// Retention: idle period records are kept for 30 days.
pub(super) const IDLE_PERIOD_RETENTION_DAYS: i64 = 30;

/// OAuth token refresh check interval (seconds).
#[cfg(feature = "analysis")]
pub(super) const OAUTH_REFRESH_INTERVAL_SECS: u64 = 120;

/// Coaching evaluation interval — 30 seconds.
pub(super) const COACHING_INTERVAL_SECS: u64 = 30;

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

#[derive(Clone)]
pub(super) struct PlatformEgressPolicy {
    enabled: bool,
    external_data_policy: ExternalDataPolicy,
    privacy_config: PrivacyConfig,
    /// 텔레메트리 동의 바인딩. 프로덕션에서 공유 `Arc<dyn ConsentManagerPort>`가 주입되며,
    /// `None`이면 기존 동작(config.upload_enabled 단독)을 유지한다.
    consent_manager: Option<Arc<dyn ConsentManagerPort>>,
}

impl PlatformEgressPolicy {
    pub(super) fn new(config: &SchedulerConfig) -> Self {
        Self {
            enabled: config.upload_enabled,
            external_data_policy: config.external_data_policy,
            privacy_config: config.privacy_config.clone(),
            consent_manager: None,
        }
    }

    /// 텔레메트리 동의(ConsentManager)를 egress 정책에 바인딩한다.
    /// 프로덕션 sync 루프에서 공유 `Arc<dyn ConsentManagerPort>`를 주입한다.
    pub(super) fn with_consent_manager(
        mut self,
        consent_manager: Option<Arc<dyn ConsentManagerPort>>,
    ) -> Self {
        self.consent_manager = consent_manager;
        self
    }

    /// 텔레메트리 동의 여부. ConsentManager 미주입 시 `true`(기존 동작 보존),
    /// 주입 시 Valid 상태 게이트를 통과한 `telemetry` 권한이 grant 된 경우에만
    /// `true`(fail-closed).
    ///
    /// `effective_permissions()`(Valid-only)를 사용한다 — raw `is_permitted`는
    /// Expired/UpdateRequired consent 의 on-disk telemetry=true 비트를 그대로
    /// 반영해 fail-open 되므로, 형제 게이트(예: system.rs)와 동일하게 Valid
    /// 게이트를 적용한다. 이렇게 해야 is_enabled()(egress 결정)와
    /// consent_state_snapshot()(ledger) 모두 만료/무효 동의에서 fail-closed 된다.
    fn telemetry_consented(&self) -> bool {
        self.consent_manager
            .as_ref()
            .is_none_or(|cm| cm.effective_permissions().telemetry)
    }

    /// egress 활성 여부 = 업로드 설정 AND 텔레메트리 동의.
    pub(super) fn is_enabled(&self) -> bool {
        self.enabled && self.telemetry_consented()
    }

    /// egress 시점의 동의 스냅샷 문자열 (egress_ledger.consent_state 용, #4803).
    ///
    /// 업로드 설정과 텔레메트리 동의 여부를 함께 기록해, 차단(blocked) 사유가
    /// 동의 부재인지 업로드 비활성인지 사후 감사로 구분할 수 있게 한다.
    pub(super) fn consent_state_snapshot(&self) -> String {
        format!(
            "upload_enabled={};telemetry={}",
            self.enabled,
            self.telemetry_consented()
        )
    }

    pub(super) fn prepare_event_for_upload(&self, mut event: Event) -> Option<Event> {
        // 업로드 설정 + 텔레메트리 동의(fail-closed)를 모두 통과해야 egress 한다.
        if !self.is_enabled() {
            return None;
        }

        match &mut event {
            Event::Context(ctx) => {
                let app_name = ctx.app_name.clone();
                let title = ctx.window_title.clone();
                if self.should_skip(&app_name, &title) {
                    return None;
                }
                ctx.window_title = self.sanitize_title(&title);
            }
            Event::Window(layout) => {
                let app_name = layout.window.app_name.clone();
                let title = layout.window.window_title.clone();
                if self.should_skip(&app_name, &title) {
                    return None;
                }
                layout.window.window_title = self.sanitize_title(&title);
            }
            Event::User(user) => {
                let app_name = user.app_name.clone();
                let title = user.window_title.clone();
                if self.should_skip(&app_name, &title) {
                    return None;
                }
                user.window_title = self.sanitize_title(&title);
            }
            Event::System(_) | Event::Input(_) | Event::Process(_) => {}
            // 클립보드/파일 접근 이벤트는 민감 데이터를 포함할 수 있으므로
            // egress 정책상 fail-closed(기본 차단)로 처리하여 업로드에서 제외한다.
            Event::Clipboard(_) | Event::FileAccess(_) => return None,
        }

        Some(event)
    }

    fn sanitize_title(&self, title: &str) -> String {
        match self.external_data_policy {
            ExternalDataPolicy::AllowFiltered => {
                sanitize_title_with_level(title, self.privacy_config.pii_filter_level)
            }
            ExternalDataPolicy::PiiFilterStrict | ExternalDataPolicy::PiiFilterStandard => {
                REDACTED_WINDOW_TITLE.to_string()
            }
        }
    }

    fn should_skip(&self, app_name: &str, window_title: &str) -> bool {
        should_exclude(
            app_name,
            window_title,
            &self.privacy_config.excluded_apps,
            &self.privacy_config.excluded_app_patterns,
            &self.privacy_config.excluded_title_patterns,
            self.privacy_config.auto_exclude_sensitive,
        )
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use maekon_core::consent::ConsentManager;
    use maekon_core::models::event::{
        ClipboardContentType, ClipboardEvent, FileAccessEvent, FileEventType,
    };
    use std::path::PathBuf;

    /// #5669: the SchedulerStorage seam must reach the real habit_streaks
    /// table — the widget reader (query_habit_streaks) must see the row the
    /// coaching loop writes through the trait object.
    #[test]
    fn upsert_habit_streak_round_trips_through_scheduler_storage_seam() {
        let storage = SqliteStorage::open_in_memory(30).expect("open_in_memory failed");
        // Same local-date key the coaching loop writes (must stay inside the
        // reader's `date >= date('now', '-N days')` window on any run date).
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        let seam: &dyn SchedulerStorage = &storage;
        seam.upsert_habit_streak("Deep Work", &today, 75, 60, true)
            .expect("trait-object upsert must reach the inherent impl");

        let rows = storage
            .query_habit_streaks(7)
            .expect("query_habit_streaks failed");
        assert_eq!(rows.len(), 1, "the seam write must land in habit_streaks");
        assert_eq!(rows[0].regime_label, "Deep Work");
        assert_eq!(rows[0].date, today);
        assert_eq!(rows[0].minutes_logged, 75);
        assert_eq!(rows[0].target_minutes, 60);
        assert!(rows[0].met);
    }

    fn enabled_policy() -> PlatformEgressPolicy {
        // 업로드가 활성화된 정책으로도 클립보드/파일 이벤트가 차단되는지 검증한다.
        let config = SchedulerConfig {
            upload_enabled: true,
            ..Default::default()
        };
        PlatformEgressPolicy::new(&config)
    }

    #[test]
    fn clipboard_event_is_dropped_from_upload() {
        let policy = enabled_policy();
        let event = Event::Clipboard(ClipboardEvent {
            timestamp: Utc::now(),
            content_type: ClipboardContentType::Text,
            char_count: 42,
            preview: Some("sensitive".to_string()),
        });

        // fail-closed: 클립보드 이벤트는 업로드에서 제외되어야 한다.
        assert!(policy.prepare_event_for_upload(event).is_none());
    }

    #[test]
    fn file_access_event_is_dropped_from_upload() {
        let policy = enabled_policy();
        let event = Event::FileAccess(FileAccessEvent {
            timestamp: Utc::now(),
            relative_path: PathBuf::from("secret/notes.txt"),
            event_type: FileEventType::Created,
            extension: Some("txt".to_string()),
        });

        // fail-closed: 파일 접근 이벤트는 업로드에서 제외되어야 한다.
        assert!(policy.prepare_event_for_upload(event).is_none());
    }

    // --- #4805: egress 를 telemetry 동의에 바인딩 ---

    fn policy_with_telemetry(consented: bool) -> PlatformEgressPolicy {
        use maekon_core::consent::ConsentPermissions;
        // grant_consent 가 in-memory 상태를 갱신하므로(파일 재독 없음) tempdir 는 즉시 drop 가능.
        let dir = tempfile::tempdir().expect("tempdir");
        let cm = Arc::new(ConsentManager::new(dir.path().join("consent.json")));
        cm.grant_consent(
            ConsentPermissions {
                telemetry: consented,
                ..Default::default()
            },
            30,
        )
        .expect("grant_consent");
        let config = SchedulerConfig {
            upload_enabled: true,
            ..Default::default()
        };
        PlatformEgressPolicy::new(&config).with_consent_manager(Some(cm))
    }

    #[test]
    fn egress_blocked_when_telemetry_consent_absent() {
        // upload_enabled=true 라도 telemetry 동의가 없으면 egress 차단(fail-closed).
        let policy = policy_with_telemetry(false);
        assert!(!policy.is_enabled());
        let event = Event::Clipboard(ClipboardEvent {
            timestamp: Utc::now(),
            content_type: ClipboardContentType::Text,
            char_count: 1,
            preview: None,
        });
        assert!(policy.prepare_event_for_upload(event).is_none());
    }

    #[test]
    fn egress_allowed_when_telemetry_consent_present() {
        // telemetry 동의 + upload_enabled 면 egress 활성.
        let policy = policy_with_telemetry(true);
        assert!(policy.is_enabled());
    }

    #[test]
    fn egress_without_consent_manager_preserves_legacy_behavior() {
        // ConsentManager 미주입 시 기존 동작(upload_enabled 단독) 유지.
        assert!(enabled_policy().is_enabled());
    }

    #[test]
    fn egress_blocked_when_telemetry_consent_expired() {
        // #4803: 만료된 동의 레코드에 telemetry=true 비트가 남아 있어도,
        // Valid 게이트(effective_permissions)를 거치므로 egress 결정과 ledger
        // 스냅샷 모두 fail-closed 되어야 한다(fail-open 회귀 방지).
        use maekon_core::consent::{ConsentPermissions, ConsentRecord, CURRENT_POLICY_VERSION};

        let dir = tempfile::tempdir().expect("tempdir");
        // 만료된 on-disk 레코드(telemetry=true)를 직접 기록한다 — consent.rs:555
        // effective_permissions_valid_gates_sub_tier_fields_when_expired 패턴 모방.
        let record = ConsentRecord {
            consent_id: "expired-telemetry".to_string(),
            version: CURRENT_POLICY_VERSION.to_string(),
            granted_at: Utc::now() - chrono::Duration::days(365),
            expires_at: Some(Utc::now() - chrono::Duration::days(1)),
            revoked_at: None,
            data_deletion_requested: false,
            erasure_nonce: None,
            permissions: ConsentPermissions {
                telemetry: true,
                ..Default::default()
            },
            data_retention_days: 30,
        };
        let path = dir.path().join("consent_expired.json");
        std::fs::write(&path, serde_json::to_string(&record).unwrap()).unwrap();
        let cm = Arc::new(ConsentManager::new(path));

        let config = SchedulerConfig {
            upload_enabled: true,
            ..Default::default()
        };
        let policy = PlatformEgressPolicy::new(&config).with_consent_manager(Some(cm));

        // egress 결정: 만료 동의 → 차단.
        assert!(
            !policy.is_enabled(),
            "expired consent must fail-closed for egress"
        );
        // ledger 스냅샷: telemetry=false 로 기록되어야 한다.
        assert!(
            policy.consent_state_snapshot().contains("telemetry=false"),
            "ledger snapshot must record telemetry=false on expired consent, got: {}",
            policy.consent_state_snapshot()
        );
    }
}
