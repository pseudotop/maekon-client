use chrono::{Datelike, Duration as ChronoDuration, LocalResult, NaiveDate, TimeZone, Utc};
use maekon_core::config_manager::ConfigManager;
use maekon_core::error::CoreError;
use maekon_core::models::activity::{ProcessSnapshot, ProcessSnapshotEntry};
use maekon_core::ports::consent_manager::{ConsentGate, ConsentManagerPort};
use maekon_core::ports::embedding_provider::EmbeddingProvider;
use maekon_core::ports::vector_store::VectorStore;
use maekon_web::{MetricsUpdate, RealtimeEvent};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};

use super::super::Scheduler;
use super::helpers::record_to_segment_summary;

/// Pure helper for the consent-gate decision of the metrics/process collection loops.
///
/// Delegates to the `capture_permitted_now` 4-term composite gate:
/// - if `config_manager` is None → fail-closed (false)
/// - if `consent_manager` is None or not in the Valid state → all-false permissions
/// - if `capture_paused` is true → false
///
/// This function is pure (no side effects), so unit tests can verify the gate
/// decision logic without constructing the full Scheduler (see the should_rearm_vad
/// pattern).
pub(super) fn collection_permitted(
    config_manager: Option<&ConfigManager>,
    consent_manager: Option<&Arc<dyn ConsentManagerPort>>,
    paused: bool,
) -> bool {
    let Some(cm) = config_manager else {
        return false;
    };
    let consent = ConsentGate::from_ref(consent_manager).permissions_snapshot();
    crate::scheduler::capture_permitted_now(&cm.snapshot(), &consent, paused)
}

/// Consent decision for user-derived embedding re-computation.
///
/// Re-embedding stale vectors can call remote embedding providers, so it must be
/// at least as strict as the collection gate and additionally require the
/// activity_pattern_learning own-field consent.
pub(super) fn embedding_reembedding_permitted(
    config_manager: Option<&ConfigManager>,
    consent_manager: Option<&Arc<dyn ConsentManagerPort>>,
    paused: bool,
) -> bool {
    collection_permitted(config_manager, consent_manager, paused)
        && ConsentGate::from_ref(consent_manager).may_learn_activity_pattern()
}

/// Pure helper for the consent-gate decision specific to the metrics collection
/// loop (F1 / Option A).
///
/// Metrics = infrastructure health (CONS-PM09 / spec §3.8 row 16) — gate only on
/// consent (telemetry) and decouple from TS/pause/active-hours. The
/// process/aggregation loops are user-activity capture, so they keep
/// `collection_permitted` (full-composite 4-term) as-is.
///
/// System metrics (CPU/memory/disk) are infrastructure-health data, not
/// user-activity capture, so this respects the CONS-PM09 / spec §3.8 row 16
/// contract that they must continue even during a Tracking-Schedule mute window.
/// Accordingly it takes neither config (TS), capture_paused, nor active-hours as
/// arguments, and looks only at `effective_permissions().telemetry` (true only in
/// the Valid state):
/// - if `consent_manager` is None or not Valid → false (fail-closed)
/// - if telemetry is false → false (own-field gate)
pub(super) fn metrics_collection_permitted(
    consent_manager: Option<&Arc<dyn ConsentManagerPort>>,
) -> bool {
    ConsentGate::from_ref(consent_manager).may_upload_telemetry()
}

const DAILY_CLAIM_PROMOTION_MARKER_KIND: &str = "daily_claim_promotion";
const DAILY_BELIEF_REVISION_MARKER_KIND: &str = "daily_belief_revision";
const STALE_VECTOR_REEMBED_BATCH_SIZE: usize = 100;
const STALE_VECTOR_REEMBED_MAX_BATCHES: usize = 100;
const WEEKLY_DIGEST_RETENTION_WEEKS: u32 = 52;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct StaleVectorReembedStats {
    batches: usize,
    fetched: u64,
    updated: u64,
    missing_or_deleted: u64,
    failed: u64,
    hit_batch_cap: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WeeklyCatchupWindow {
    start_date: NaiveDate,
    end_date: NaiveDate,
}

async fn reembed_stale_vectors(
    vector_store: &dyn VectorStore,
    embedding_provider: &dyn EmbeddingProvider,
    batch_size: usize,
    max_batches: usize,
) -> StaleVectorReembedStats {
    let batch_size = batch_size.max(1);
    let max_batches = max_batches.max(1);
    let mut stats = StaleVectorReembedStats::default();

    for batch_index in 0..max_batches {
        match vector_store.get_stale_vectors(batch_size).await {
            Ok(batch) if !batch.is_empty() => {
                stats.batches += 1;
                stats.fetched += batch.len() as u64;

                let texts: Vec<String> = batch.iter().map(|(_, text)| text.clone()).collect();
                match embedding_provider.embed_batch(&texts).await {
                    Ok(vectors) => {
                        if vectors.len() != batch.len() {
                            warn!(
                                stale_count = batch.len(),
                                vector_count = vectors.len(),
                                "re-embed batch returned a mismatched vector count"
                            );
                        }

                        let model_id = embedding_provider.model_id();
                        let mut batch_updated = 0u64;
                        let mut batch_missing_or_deleted = 0u64;
                        let mut batch_failed = 0u64;

                        for ((id, _), vector) in batch.into_iter().zip(vectors) {
                            match vector_store.update_vector(id, vector, model_id).await {
                                Ok(rows) if rows > 0 => {
                                    batch_updated += rows;
                                    stats.updated += rows;
                                }
                                Ok(_) => {
                                    batch_missing_or_deleted += 1;
                                    stats.missing_or_deleted += 1;
                                }
                                Err(e) => {
                                    batch_failed += 1;
                                    stats.failed += 1;
                                    warn!("re-embed update failure: {e}");
                                }
                            }
                        }

                        debug!(
                            updated = batch_updated,
                            missing_or_deleted = batch_missing_or_deleted,
                            failed = batch_failed,
                            "re-embedded stale vector batch"
                        );

                        if batch_updated + batch_missing_or_deleted == 0 {
                            warn!(
                                failed = batch_failed,
                                "stale vector re-embed made no progress; stopping sweep"
                            );
                            break;
                        }
                    }
                    Err(e) => {
                        warn!("re-embed batch failure: {e}");
                        break;
                    }
                }
            }
            Ok(_) => break,
            Err(e) => {
                warn!("get stale vectors failure: {e}");
                break;
            }
        }

        if batch_index + 1 == max_batches {
            stats.hit_batch_cap = true;
            warn!(
                max_batches,
                fetched = stats.fetched,
                updated = stats.updated,
                missing_or_deleted = stats.missing_or_deleted,
                failed = stats.failed,
                "stale vector re-embed hit batch cap; remaining stale vectors will be retried later"
            );
        }
    }

    stats
}

fn daily_catchup_dates(
    local_today: NaiveDate,
    latest_digest_date: Option<NaiveDate>,
    retention_days: u32,
) -> Vec<NaiveDate> {
    let yesterday = local_today.pred_opt().unwrap_or(local_today);
    let retention_days = retention_days.max(1);
    let oldest_allowed = yesterday
        .checked_sub_signed(ChronoDuration::days((retention_days - 1) as i64))
        .unwrap_or(yesterday);
    let mut start = latest_digest_date
        .map(|date| date.max(oldest_allowed))
        .unwrap_or(yesterday);
    if start > yesterday {
        start = yesterday;
    }

    inclusive_date_range(start, yesterday, 1)
}

fn weekly_catchup_window_dates(
    local_today: NaiveDate,
    digest_day: maekon_core::config::Weekday,
    latest_week_start_date: Option<NaiveDate>,
    retention_weeks: u32,
) -> Vec<WeeklyCatchupWindow> {
    let days_since_digest_day = (local_today.weekday().num_days_from_sunday() as i64
        - digest_day.num_days_from_sunday() as i64)
        .rem_euclid(7);
    let target_end = local_today
        .checked_sub_signed(ChronoDuration::days(days_since_digest_day))
        .unwrap_or(local_today);
    let target_start = target_end
        .checked_sub_signed(ChronoDuration::days(7))
        .unwrap_or(target_end);

    let retention_weeks = retention_weeks.max(1);
    let oldest_start = target_start
        .checked_sub_signed(ChronoDuration::days(((retention_weeks - 1) * 7) as i64))
        .unwrap_or(target_start);
    let start = latest_week_start_date
        .and_then(|date| date.checked_add_signed(ChronoDuration::days(7)))
        .map(|date| date.max(oldest_start))
        .unwrap_or(target_start);

    if start > target_start {
        return Vec::new();
    }

    inclusive_date_range(start, target_start, 7)
        .into_iter()
        .map(|start_date| WeeklyCatchupWindow {
            start_date,
            end_date: start_date
                .checked_add_signed(ChronoDuration::days(7))
                .unwrap_or(start_date),
        })
        .collect()
}

fn inclusive_date_range(start: NaiveDate, end: NaiveDate, step_days: i64) -> Vec<NaiveDate> {
    let mut out = Vec::new();
    let mut cursor = start;
    let step_days = step_days.max(1);
    while cursor <= end {
        out.push(cursor);
        let Some(next) = cursor.checked_add_signed(ChronoDuration::days(step_days)) else {
            break;
        };
        cursor = next;
    }
    out
}

fn local_midnight_utc(date: NaiveDate) -> Option<chrono::DateTime<Utc>> {
    let local_midnight = date.and_hms_opt(0, 0, 0)?;
    match chrono::Local.from_local_datetime(&local_midnight) {
        LocalResult::Single(dt) => Some(dt.with_timezone(&Utc)),
        LocalResult::Ambiguous(first, second) => {
            let first = first.with_timezone(&Utc);
            let second = second.with_timezone(&Utc);
            Some(first.min(second))
        }
        LocalResult::None => {
            for hour in 1..4 {
                let Some(candidate) = date.and_hms_opt(hour, 0, 0) else {
                    continue;
                };
                match chrono::Local.from_local_datetime(&candidate) {
                    LocalResult::Single(dt) => return Some(dt.with_timezone(&Utc)),
                    LocalResult::Ambiguous(first, second) => {
                        let first = first.with_timezone(&Utc);
                        let second = second.with_timezone(&Utc);
                        return Some(first.min(second));
                    }
                    LocalResult::None => {}
                }
            }
            None
        }
    }
}

impl Scheduler {
    #[tracing::instrument(skip_all)]
    pub(in crate::scheduler) fn spawn_metrics_loop(
        &self,
        metrics_interval: Duration,
        mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) -> tokio::task::JoinHandle<()> {
        let sys_mon = self.system_monitor.clone();
        let sqlite2 = self.sqlite_storage.clone();
        let event_tx2 = self.event_tx.clone();
        let notif2 = self.notification_manager.clone();
        // Consent-gate DI — metrics collection/persistence cannot run without
        // telemetry consent. Metrics = infrastructure health (CONS-PM09 / spec §3.8
        // row 16), so gate only on consent (telemetry) and decouple from
        // TS/pause/active-hours (no config_manager / capture_paused clone needed).
        // The process/aggregation loops keep the full-composite gate.
        let consent_manager_m = self.consent_manager.clone();

        tokio::spawn(async move {
            let mut interval = super::intervals::coalescing_interval(metrics_interval);
            // Decoupled power-status polling gate — pmset has a fork/exec cost, so we
            // refresh only every ~60s rather than calling it on every metrics tick
            // (~5s). Between refreshes we cache and re-apply the last measured status,
            // so the battery-saver flag always reflects the latest measurement (same
            // num_minutes gate pattern as last_index_maintenance).
            let mut last_power_check: Option<chrono::DateTime<Utc>> = None;
            let mut cached_power_status = maekon_core::models::system::PowerStatus::default();

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        // Power-status refresh — an operational task that sets the
                        // battery-saver flag, so it always runs regardless of the
                        // consent gate. To save pmset fork/exec cost, actual polling
                        // happens only every ~60s; between polls we re-apply the cache.
                        let now = Utc::now();
                        let should_poll_power = last_power_check
                            .map(|last| (now - last).num_minutes() >= 1)
                            .unwrap_or(true);
                        if should_poll_power {
                            match sys_mon.current_power_status().await {
                                Ok(status) => {
                                    cached_power_status = status;
                                    last_power_check = Some(now);
                                    debug!(
                                        external_power_connected = ?cached_power_status.external_power_connected,
                                        battery_percent = ?cached_power_status.battery_percent,
                                        low_battery = cached_power_status.low_battery,
                                        battery_saver_active = cached_power_status.battery_saver_active,
                                        "power status updated"
                                    );
                                }
                                Err(e) => {
                                    warn!("power status collect failure: {e}");
                                }
                            }
                        }
                        // Re-apply the latest (cached) measurement to the scheduler
                        // flag on every tick.
                        crate::scheduler::set_battery_saver_active_for_scheduler(
                            cached_power_status.battery_saver_active,
                        );
                        // Metrics collection/persistence block — protected by the
                        // consent (telemetry) gate alone. This is infrastructure-health
                        // data (CONS-PM09 / spec §3.8 row 16), so it is decoupled from
                        // TS/pause/active-hours — only telemetry consent is checked.
                        let permitted = metrics_collection_permitted(consent_manager_m.as_ref());
                        if permitted {
                            match sys_mon.collect_metrics().await {
                                Ok(metrics) => {
                                    if let Err(e) = sqlite2.save_metrics(&metrics).await {
                                        warn!("system save failure: {e}");
                                    }

                                    let memory_percent = if metrics.memory_total > 0 {
                                        (metrics.memory_used as f32 / metrics.memory_total as f32)
                                            * 100.0
                                    } else {
                                        0.0
                                    };

                                    if let Some(ref tx) = event_tx2 {
                                        let update = MetricsUpdate {
                                            timestamp: metrics.timestamp.to_rfc3339(),
                                            cpu_usage: metrics.cpu_usage,
                                            memory_percent,
                                            memory_used: metrics.memory_used,
                                            memory_total: metrics.memory_total,
                                        };
                                        if let Err(e) = tx.send(RealtimeEvent::Metrics(update)) {
                                            debug!("channel send failed: {e}");
                                        }
                                    }

                                    if let Some(ref notif) = notif2 {
                                        notif.check_high_usage(metrics.cpu_usage, memory_percent)
                                            .await;
                                    }
                                }
                                Err(e) => {
                                    warn!("system collect failure: {e}");
                                }
                            }
                        }
                    }
                    _ = shutdown_rx.changed() => {
                        info!("ended");
                        break;
                    }
                }
            }
        })
    }

    #[tracing::instrument(skip_all)]
    pub(in crate::scheduler) fn spawn_process_loop(
        &self,
        process_interval: Duration,
        mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) -> tokio::task::JoinHandle<()> {
        let proc_mon = self.process_monitor.clone();
        let sqlite3 = self.sqlite_storage.clone();
        // Consent-gate DI — process snapshots are user data, so they cannot be
        // persisted without consent.
        let config_manager_p = self.config_manager.clone();
        let consent_manager_p = self.consent_manager.clone();
        let capture_paused_p = self.capture_paused.clone();

        tokio::spawn(async move {
            let mut interval = super::intervals::coalescing_interval(process_interval);

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        // 4-term composite gate — without consent, skip process-list
                        // collection/persistence.
                        let permitted = collection_permitted(
                            config_manager_p.as_ref(),
                            consent_manager_p.as_ref(),
                            capture_paused_p.load(Ordering::Relaxed),
                        );
                        if permitted {
                            match proc_mon.get_top_processes(10).await {
                                Ok(processes) => {
                                    let snapshot = ProcessSnapshot {
                                        timestamp: Utc::now(),
                                        processes: processes
                                            .into_iter()
                                            .map(|p| ProcessSnapshotEntry {
                                                pid: p.pid,
                                                name: p.name,
                                                cpu_usage: p.cpu_usage,
                                                memory_bytes: p.memory_bytes,
                                            })
                                            .collect(),
                                    };
                                    if let Err(e) = sqlite3.save_process_snapshot(&snapshot).await {
                                        warn!("save failure: {e}");
                                    }
                                }
                                Err(e) => {
                                    warn!("list collect failure: {e}");
                                }
                            }
                        }
                    }
                    _ = shutdown_rx.changed() => {
                        info!("ended");
                        break;
                    }
                }
            }
        })
    }

    #[tracing::instrument(skip_all)]
    pub(in crate::scheduler) fn spawn_aggregation_loop(
        &self,
        aggregation_interval: Duration,
        llm_summarizer: Option<Arc<maekon_analysis::LlmSegmentSummarizer>>,
        mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) -> tokio::task::JoinHandle<()> {
        let sqlite6 = self.sqlite_storage.clone();
        let memory_graph = self.memory_graph.clone();
        let belief_revision = self.belief_revision.clone();
        let consent_manager = self.consent_manager.clone();
        let vector_store = self.vector_store.clone();
        let embedding_provider = self.embedding_provider.clone();
        let config_manager = self.config_manager.clone();
        // Consent-gate DI — the aggregation loop's data derivation/persistence
        // (hourly aggregation, daily/weekly digests, memory-graph claims, stale
        // vector re-embedding) cannot run without consent (CONS-PC02). Housekeeping
        // (retention deletes, sqlite maintenance, index maintenance, log cleanup)
        // always runs outside the gate.
        let capture_paused = self.capture_paused.clone();
        let vector_index = self.vector_index.clone();
        let search_coordinator = self.search_coordinator.clone();
        #[cfg(feature = "hnsw")]
        let ann_index = self.ann_index.clone();
        // #5810: regime crash-durability checkpoint — same Arcs as shutdown path.
        let regime_storage = self.regime_storage.clone();
        let regime_manager_arc = self.regime_manager_arc.clone();
        // #7678 D4: daily-summary desktop toast (previously-inert
        // `daily_summary_notification` config flag) — same Arc as every other
        // loop's notification_manager (Port Instance Sharing guardrail).
        let notification_manager = self.notification_manager.clone();
        // #7678 D4: calibration-log retention (previously-inert
        // `calibration_retention_days`/`calibration_max_rows` config fields) —
        // same underlying SqliteStorage instance as calibration_writer/reader.
        let calibration_reader = self.calibration_reader.clone();

        // Resolve log directory once for periodic log retention cleanup.
        let log_dir = maekon_core::config_manager::ConfigManager::data_dir()
            .map(|d| d.join("logs"))
            .ok();

        // Config file mtime tracker — shared into the spawned task.
        let config_mtime: Arc<parking_lot::Mutex<Option<std::time::SystemTime>>> =
            Arc::new(parking_lot::Mutex::new(None));

        tokio::spawn(async move {
            // #6441 F18: evict the local embedding model after this long idle so an
            // enabled-but-unused provider does not pin ONNX RSS for the process
            // lifetime. Generous enough that an actively-used model (analysis
            // pipeline embeds) is never evicted mid-use; the next embed reloads it.
            const EMBEDDING_IDLE_EVICT: Duration = Duration::from_secs(600);
            let mut interval = super::intervals::coalescing_interval(aggregation_interval);
            let mut last_reindex_check: Option<chrono::DateTime<Utc>> = None;
            let mut last_index_maintenance: Option<chrono::DateTime<Utc>> = None;
            let mut last_log_cleanup: Option<chrono::DateTime<Utc>> = None;
            let mut last_sqlite_maintenance: Option<chrono::DateTime<Utc>> = None;
            let mut last_fts_optimize: Option<chrono::DateTime<Utc>> = None;
            // #5810/#7574: regime crash-durability checkpoint runs on its own
            // sub-tick timer, independent of `aggregation_interval` (default
            // 60 min) — see `regime_checkpoint_interval` below. Previously this
            // fired only inside the `interval.tick()` (aggregation) branch, so
            // the ">= REGIME_CHECKPOINT_INTERVAL_MINS" gate could only ever be
            // evaluated once per hour and the documented "at most 30 min lost on
            // unclean exit" bound was actually ~60 min.
            let mut regime_checkpoint_interval =
                super::intervals::coalescing_interval(Duration::from_secs(
                    super::super::config::REGIME_CHECKPOINT_INTERVAL_MINS as u64 * 60,
                ));

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        let now = Utc::now();

                        // Compute the 4-term composite consent gate once per tick
                        // (reusing the R4 helper). This is a free fn in the same module
                        // (system.rs), so it is called without a path prefix. When
                        // collect_ok == false we skip the data derivation/persistence
                        // blocks, while housekeeping (retention deletes, maintenance)
                        // keeps running regardless of the gate — because even during a
                        // consent gap, retention policy must still clean up expired data.
                        let collect_ok = collection_permitted(
                            config_manager.as_ref(),
                            consent_manager.as_ref(),
                            capture_paused.load(Ordering::Relaxed),
                        );
                        let embedding_reindex_ok = embedding_reembedding_permitted(
                            config_manager.as_ref(),
                            consent_manager.as_ref(),
                            capture_paused.load(Ordering::Relaxed),
                        );

                        // [HOUSEKEEPING] #6441 F18: reclaim ONNX RSS by evicting the
                        // local embedding model when it has been idle. No-op for
                        // remote/no-op providers (default trait impl) and when nothing
                        // is resident. Memory-only → runs outside the consent gate.
                        if let Some(ref ep) = embedding_provider {
                            ep.evict_if_idle(EMBEDDING_IDLE_EVICT);
                        }

                        // [COLLECT] Hourly metric aggregation — derive and persist
                        // rollups from raw metrics.
                        if collect_ok {
                            let prev_hour = now - ChronoDuration::hours(1);
                            if let Err(e) = sqlite6.aggregate_hourly_metrics(prev_hour).await {
                                warn!("hour failure: {e}");
                            }
                        }

                        // [HOUSEKEEPING] The three cleanup_* calls below only delete
                        // expired data per the retention policy (not collection). They
                        // must run even during a consent gap, so they sit outside the gate.
                        //
                        // #4631 MINOR-3 (intentional gap): the rollup
                        // (aggregate_hourly_metrics) is inside the consent gate, but
                        // cleanup_old_metrics (delete-only) is outside it. During a 23h+
                        // continuous consent gap, the raw metrics from the prior consent
                        // window may exceed the retention deadline and be deleted before
                        // they are rolled up, leaving the hourly rollup for that period
                        // permanently absent. This is accepted because it is fail-closed
                        // behavior that does not derive from withdrawn-consent data
                        // (self-induced withdrawal).
                        let metrics_cutoff = now - ChronoDuration::hours(super::super::config::RAW_METRICS_RETENTION_HOURS);
                        if let Err(e) = sqlite6.cleanup_old_metrics(metrics_cutoff).await {
                            warn!("delete failure: {e}");
                        }

                        // Hourly rollups have a 30-day retention (V3 migration) but
                        // previously had no scheduled cleanup, so the table grew
                        // unbounded. Delete-only housekeeping → outside the consent gate.
                        let hourly_cutoff = now - ChronoDuration::days(super::super::config::HOURLY_METRICS_RETENTION_DAYS);
                        if let Err(e) = sqlite6.cleanup_old_hourly_metrics(hourly_cutoff).await {
                            warn!("hourly metrics delete failure: {e}");
                        }

                        let process_cutoff = now - ChronoDuration::days(super::super::config::PROCESS_SNAPSHOT_RETENTION_DAYS);
                        if let Err(e) = sqlite6.cleanup_old_process_snapshots(process_cutoff).await {
                            warn!("delete failure: {e}");
                        }

                        let idle_cutoff = now - ChronoDuration::days(super::super::config::IDLE_PERIOD_RETENTION_DAYS);
                        if let Err(e) = sqlite6.cleanup_old_idle_periods(idle_cutoff).await {
                            warn!("idle period delete failure: {e}");
                        }

                        // --- Embedding re-indexing on model version change (daily) ---
                        if let (Some(ref vs), Some(ref ep)) = (&vector_store, &embedding_provider) {
                            let should_check = last_reindex_check
                                .map(|last| (now - last).num_hours() >= 24)
                                .unwrap_or(true);

                            if should_check {
                                last_reindex_check = Some(now);

                                // [COLLECT] Stale vector re-embedding — on a model
                                // change, re-derive embeddings (update_vector) and
                                // persist this user-derived data. Protected by the
                                // consent gate. (mark_stale itself is just a flag, but it
                                // is the entry point of the re-embedding pipeline, so it
                                // is enclosed together.)
                                if embedding_reindex_ok {
                                    let config_model = config_manager
                                        .as_ref()
                                        .map(|cm| cm.get().analysis.embedding.local_model.clone())
                                        .unwrap_or_default();

                                    match vs.get_current_model_id().await {
                                        Ok(Some(stored_model)) if !config_model.is_empty() && stored_model != config_model => {
                                            info!(
                                                old_model = %stored_model,
                                                new_model = %config_model,
                                                "Embedding model changed — marking old vectors stale"
                                            );
                                            if let Err(e) = vs.mark_stale(&stored_model).await {
                                                warn!("mark stale failure: {e}");
                                            }
                                        }
                                        _ => {}
                                    }

                                    let stats = reembed_stale_vectors(
                                        vs.as_ref(),
                                        ep.as_ref(),
                                        STALE_VECTOR_REEMBED_BATCH_SIZE,
                                        STALE_VECTOR_REEMBED_MAX_BATCHES,
                                    )
                                    .await;
                                    if stats.fetched > 0 {
                                        debug!(
                                            fetched = stats.fetched,
                                            updated = stats.updated,
                                            missing_or_deleted = stats.missing_or_deleted,
                                            failed = stats.failed,
                                            hit_batch_cap = stats.hit_batch_cap,
                                            "stale vector re-embed sweep finished"
                                        );
                                    }
                                }

                                // [HOUSEKEEPING] Vector retention — only deletes expired
                                // vectors from HNSW/SQLite (not collection). Must clean up
                                // per the retention policy even during a consent gap, so
                                // it sits outside the gate.
                                // Enforce vector retention (HNSW removal + SQLite deletion)
                                let retention_days = config_manager
                                    .as_ref()
                                    .map(|cm| cm.get().analysis.embedding.retention_days)
                                    .unwrap_or(90);

                                // Best-effort: remove expired vectors from HNSW before SQLite deletes them
                                #[cfg(feature = "hnsw")]
                                if let Some(ref ann) = ann_index {
                                    match vs.get_expired_ids(retention_days).await {
                                        Ok(ids) if !ids.is_empty() => {
                                            let mut removed = 0u64;
                                            for id in &ids {
                                                if let Err(e) = ann.remove(*id).await {
                                                    warn!("HNSW remove key={id} failed (best-effort): {e}");
                                                } else {
                                                    removed += 1;
                                                }
                                            }
                                            if removed > 0 {
                                                debug!("Removed {removed}/{} expired vectors from HNSW index", ids.len());
                                            }
                                        }
                                        Ok(_) => {} // no expired IDs
                                        Err(e) => {
                                            warn!("get_expired_ids failed (best-effort): {e}");
                                        }
                                    }
                                }

                                if let Err(e) = vs.enforce_retention(retention_days).await {
                                    warn!("vector retention failure: {e}");
                                }
                            }
                        }

                        // [HOUSEKEEPING] Segment/digest/auxiliary-table retention +
                        // memory-graph prune/GC. All only delete/clean up expired data
                        // (not collection). The retention policy must apply even during a
                        // consent gap, so this runs outside the gate.
                        // --- Activity segment retention (default: 90 days, same as embedding) ---
                        {
                            let segment_retention_days = config_manager
                                .as_ref()
                                .map(|cm| cm.get().analysis.embedding.retention_days)
                                .unwrap_or(90);
                            // F-RR-06 (#5097/#5809 follow-up): these four sync
                            // SchedulerStorage retention DELETEs acquire the parking_lot
                            // write lock — offload to the blocking pool (mirror the digest
                            // lookups above + the maintenance blocks below) so the
                            // write-lock DELETE batch never runs on the reactor.
                            // Best-effort: offload_storage logs failures and returns None.
                            {
                                let sqlite6 = sqlite6.clone();
                                offload_storage("segment retention", move || {
                                    sqlite6.enforce_segment_retention(segment_retention_days)
                                })
                                .await;
                            }

                            // Weekly digests retention (keep 52 weeks = 1 year)
                            {
                                let sqlite6 = sqlite6.clone();
                                offload_storage("digest retention", move || {
                                    sqlite6.enforce_digest_retention(52)
                                })
                                .await;
                            }

                            // Auxiliary table retention (work_sessions, interruptions, etc.)
                            {
                                let sqlite6 = sqlite6.clone();
                                offload_storage("auxiliary table retention", move || {
                                    sqlite6.enforce_all_retention()
                                })
                                .await;
                            }

                            // #8056 P3: compliance-window age cap on the security
                            // audit trails (audit_log + session_audit_log). These are
                            // excluded from enforce_all_retention and RETAINED across
                            // erasure, so they need their own bounded prune. audit_log
                            // is pruned chain-safely (ADR-072 tamper-evidence preserved).
                            {
                                let sqlite6 = sqlite6.clone();
                                offload_storage("audit-trail retention", move || {
                                    sqlite6.enforce_audit_retention()
                                })
                                .await;
                            }

                            // GDPR Art.17 erasure tombstone outbox GC (#5174 S5/R4):
                            // bound the retained sync_tombstones at max(retention, 90)
                            // days. Accepted convergence-cliff trade-off (see method doc).
                            {
                                let sqlite6 = sqlite6.clone();
                                offload_storage("sync_tombstones GC", move || {
                                    sqlite6.gc_sync_tombstones(segment_retention_days)
                                })
                                .await;
                            }

                            // #7678 D4: calibration_log retention — previously the
                            // configured `calibration_retention_days`/`calibration_max_rows`
                            // were parsed+persisted but never enforced, so the table grew
                            // unbounded. `CalibrationReader::enforce_retention` is already
                            // async (ADR-026 convergence), so no `offload_storage` wrapper
                            // is needed here (mirrors the VectorStore retention call above).
                            if let Some(ref cr) = calibration_reader {
                                let (max_days, max_rows) = config_manager
                                    .as_ref()
                                    .map(|cm| {
                                        let tm = &cm.get().analysis.tiered_memory;
                                        (tm.calibration_retention_days, tm.calibration_max_rows)
                                    })
                                    .unwrap_or((14, 500_000));
                                match cr.enforce_retention(max_days, max_rows).await {
                                    Ok(n) if n > 0 => {
                                        debug!("calibration_log: pruned {n} row(s)")
                                    }
                                    Ok(_) => {}
                                    Err(e) => {
                                        warn!(err.code = %e.code(), "calibration_log retention failure: {e}")
                                    }
                                }
                            }

                            // ADR-023: bound the memory-graph (same retention window as
                            // segments) + GC evidence edges orphaned by segment deletion.
                            if let Some(ref mg) = memory_graph {
                                let cutoff = (now
                                    - ChronoDuration::days(segment_retention_days as i64))
                                .timestamp();
                                match mg.prune_claims_older_than(cutoff).await {
                                    Ok(n) if n > 0 => debug!("ADR-023: pruned {n} memory claim(s)"),
                                    Ok(_) => {}
                                    Err(e) => {
                                        warn!(err.code = %e.code(), "memory-graph claim retention failure: {e}")
                                    }
                                }
                                match mg.prune_orphan_evidence_edges().await {
                                    Ok(n) if n > 0 => {
                                        debug!("ADR-023: GC'd {n} orphan evidence edge(s)")
                                    }
                                    Ok(_) => {}
                                    Err(e) => {
                                        warn!(err.code = %e.code(), "memory-graph orphan-edge GC failure: {e}")
                                    }
                                }
                            }
                        }

                        // [COLLECT] Weekly digest auto-generation — derive a digest from
                        // the segments and persist it via save_weekly_digest. Protected
                        // by the consent gate.
                        // --- Weekly digest catch-up generation ---
                        if collect_ok {
                            let digest_day = config_manager
                                .as_ref()
                                .map(|cm| cm.get().analysis.embedding.digest_day)
                                .unwrap_or(maekon_core::config::Weekday::Sun);
                            let local_today = chrono::Local::now().date_naive();

                            // #5097: offload sync SchedulerStorage digest calls to
                            // spawn_blocking (offload_storage) — avoid blocking the
                            // async worker thread.
                            let mut previous_digest = {
                                let sqlite6 = sqlite6.clone();
                                offload_storage("weekly digest lookup", move || {
                                    sqlite6.list_weekly_digests(1)
                                })
                                .await
                            }
                            .and_then(|d| d.into_iter().next());

                            let latest_week_start_date = previous_digest
                                .as_ref()
                                .map(|digest| digest.week_start.with_timezone(&chrono::Local).date_naive());
                            let windows = weekly_catchup_window_dates(
                                local_today,
                                digest_day,
                                latest_week_start_date,
                                WEEKLY_DIGEST_RETENTION_WEEKS,
                            );

                            for window in windows {
                                let Some(week_start) = local_midnight_utc(window.start_date) else {
                                    warn!("weekly digest catch-up skipped: invalid local start date {}", window.start_date);
                                    continue;
                                };
                                let Some(week_end) = local_midnight_utc(window.end_date) else {
                                    warn!("weekly digest catch-up skipped: invalid local end date {}", window.end_date);
                                    continue;
                                };

                                // Load actual segments for this week from storage.
                                let week_segments = {
                                    let sqlite6 = sqlite6.clone();
                                    offload_storage("weekly segments load", move || {
                                        sqlite6.list_segments_between(week_start, week_end)
                                    })
                                    .await
                                }
                                .unwrap_or_default();
                                let digest = maekon_analysis::WeeklyDigestGenerator::generate(
                                    &week_segments,
                                    week_start,
                                    week_end,
                                    previous_digest.as_ref(),
                                );

                                let saved = {
                                    let sqlite6 = sqlite6.clone();
                                    let digest = digest.clone();
                                    offload_storage("weekly digest save", move || {
                                        sqlite6.save_weekly_digest(&digest)
                                    })
                                    .await
                                };
                                if saved.is_some() {
                                    info!("Weekly digest generated for week ending {}", week_end);
                                    previous_digest = Some(digest);
                                }
                            }
                        }

                        // [COLLECT] Daily digest auto-generation — derive a digest from
                        // the segments (save_daily_digest) + promote memory-graph
                        // claim/evidence edges (persist_digest_memory_graph) + a belief
                        // revision pass. All persist user-derived data, so they are
                        // protected by the consent gate. (Belief revision is additionally
                        // gated internally on the memory_graph_enrichment own-field
                        // consent — defense-in-depth.)
                        // --- Daily digest catch-up generation ---
                        if collect_ok {
                            let local_today = chrono::Local::now().date_naive();
                            let segment_retention_days = config_manager
                                .as_ref()
                                .map(|cm| cm.get().analysis.embedding.retention_days)
                                .unwrap_or(90);
                            let latest_digest_date = {
                                let sqlite6 = sqlite6.clone();
                                offload_storage("daily digest list", move || {
                                    sqlite6.list_daily_digests(1)
                                })
                                .await
                            }
                            .and_then(|digests| digests.into_iter().next())
                            .map(|digest| digest.date);

                            for digest_date in daily_catchup_dates(
                                local_today,
                                latest_digest_date,
                                segment_retention_days,
                            ) {
                                let date_str = digest_date.format("%Y-%m-%d").to_string();

                                // Check if daily digest already exists (#5097: spawn_blocking offload).
                                let existing = {
                                    let sqlite6 = sqlite6.clone();
                                    let date_str = date_str.clone();
                                    offload_storage("daily digest lookup", move || {
                                        sqlite6.get_daily_digest(&date_str)
                                    })
                                    .await
                                }
                                .flatten();

                                let digest = if let Some(digest) = existing {
                                    Some(digest)
                                } else {
                                    // Load segments for the completed local day.
                                    let segment_records = {
                                        let sqlite6 = sqlite6.clone();
                                        let date_str = date_str.clone();
                                        offload_storage("daily segments load", move || {
                                            sqlite6.get_segments_for_date(&date_str)
                                        })
                                        .await
                                    }
                                    .unwrap_or_default();

                                    if !segment_records.is_empty() {
                                        // Convert SegmentSummaryRecords to SegmentSummary for DailyDigestGenerator
                                        let segments: Vec<maekon_core::models::tiered_memory::SegmentSummary> =
                                            segment_records
                                                .iter()
                                                .filter_map(record_to_segment_summary)
                                                .collect();

                                        // Load previous day for comparison
                                        let prev_date = digest_date
                                            .pred_opt()
                                            .unwrap_or(digest_date)
                                            .format("%Y-%m-%d")
                                            .to_string();
                                        let prev_digest = {
                                            let sqlite6 = sqlite6.clone();
                                            offload_storage("prev daily digest lookup", move || {
                                                sqlite6.get_daily_digest(&prev_date)
                                            })
                                            .await
                                        }
                                        .flatten();

                                        // #7678 D2: resolve human regime labels (name >
                                        // auto_label) from the current regime manager
                                        // snapshot so the digest timeline never leaks
                                        // the opaque `regime_id` ("regime-N") — mirrors
                                        // the #7480 coaching-path fix. Best-effort: a
                                        // regime evicted/archived since the segment was
                                        // recorded simply falls back to
                                        // `dominant_category` inside the generator.
                                        let regimes: Vec<maekon_core::models::tiered_memory::Regime> =
                                            regime_manager_arc
                                                .as_ref()
                                                .map(|m| m.lock().all_regimes().to_vec())
                                                .unwrap_or_default();
                                        let mut digest = maekon_analysis::DailyDigestGenerator::generate(
                                            &segments,
                                            digest_date,
                                            prev_digest.as_ref(),
                                            &regimes,
                                        );

                                        // Generate LLM narrative insight if provider is available.
                                        if let Some(ref summarizer) = llm_summarizer {
                                            let pii_level = config_manager
                                                .as_ref()
                                                .map(|cm| cm.get().privacy.pii_filter_level)
                                                .unwrap_or(maekon_core::config::PiiFilterLevel::Standard);
                                            let pii_filter: maekon_analysis::PiiFilter =
                                                Box::new(move |text: &str| {
                                                    maekon_vision::privacy::sanitize_title_with_level(text, pii_level)
                                                });
                                            let insight_gen = maekon_analysis::DailyInsightGenerator::new(
                                                summarizer.analysis_provider(),
                                                pii_filter,
                                            );
                                            match insight_gen.generate(&digest).await {
                                                Some(insight) => {
                                                    debug!("LLM daily insight generated for {}", date_str);
                                                    digest.insight = Some(insight);
                                                }
                                                None => {
                                                    debug!("LLM daily insight unavailable for {}", date_str);
                                                }
                                            }
                                        }

                                        let saved = {
                                            let sqlite6 = sqlite6.clone();
                                            let digest = digest.clone();
                                            offload_storage("daily digest save", move || {
                                                sqlite6.save_daily_digest(&digest)
                                            })
                                            .await
                                        };
                                        if saved.is_some() {
                                            info!("Daily digest generated for {}", date_str);
                                            // #7678 D4: fire the (previously-inert)
                                            // daily_summary_notification desktop toast
                                            // for a freshly generated digest only —
                                            // never for a cache hit (the `existing`
                                            // branch above never reaches this block).
                                            if let Some(ref nm) = notification_manager {
                                                nm.notify_daily_summary(&date_str).await;
                                            }
                                            Some(digest)
                                        } else {
                                            None
                                        }
                                    } else {
                                        None
                                    }
                                };

                                if let Some(digest) = digest {
                                        // ADR-023 (D3/D5): promote the digest into
                                        // durable memory-graph claims + evidence edges.
                                        // Offline-capable — runs on the timeline content
                                        // even when no LLM insight was generated.
                                        if let Some(ref mg) = memory_graph {
                                            let marker_exists = {
                                                let sqlite6 = sqlite6.clone();
                                                let date_str = date_str.clone();
                                                offload_storage("daily claim marker lookup", move || {
                                                    sqlite6.has_digest_processing_marker(
                                                        DAILY_CLAIM_PROMOTION_MARKER_KIND,
                                                        &date_str,
                                                    )
                                                })
                                                .await
                                            }
                                            .unwrap_or(false);

                                            if !marker_exists {
                                                let promoted = persist_digest_memory_graph(
                                                    mg.as_ref(),
                                                    &digest,
                                                    Utc::now().timestamp(),
                                                )
                                                .await;
                                                if !promoted {
                                                    continue;
                                                }
                                                let saved_marker = {
                                                    let sqlite6 = sqlite6.clone();
                                                    let date_str = date_str.clone();
                                                    offload_storage("daily claim marker save", move || {
                                                        sqlite6.save_digest_processing_marker(
                                                            DAILY_CLAIM_PROMOTION_MARKER_KIND,
                                                            &date_str,
                                                            Utc::now(),
                                                        )
                                                    })
                                                    .await
                                                };
                                                if saved_marker.is_some() {
                                                    debug!("ADR-023: daily claim promotion marked for {}", date_str);
                                                }
                                            }
                                        }

                                        // ADR-023 Phase-2: LLM belief revision (D1/D2)
                                        // over the accumulated claims, once per day with
                                        // the digest. Triple-gated: the component is
                                        // local-LLM-gated at construction; here we also
                                        // require explicit memory_graph_enrichment consent
                                        // + the belief_revision_enabled flag. With no LLM
                                        // it degrades to a no-op.
                                        if let Some(ref br) = belief_revision {
                                            let consent_ok =
                                                ConsentGate::from_ref(consent_manager.as_ref())
                                                    .may_enrich_memory_graph();
                                            let flag_on = config_manager
                                                .as_ref()
                                                .map(|cm| cm.get().analysis.belief_revision_enabled)
                                                .unwrap_or(false);
                                            if consent_ok && flag_on {
                                                let marker_exists = {
                                                    let sqlite6 = sqlite6.clone();
                                                    let date_str = date_str.clone();
                                                    offload_storage("daily belief marker lookup", move || {
                                                        sqlite6.has_digest_processing_marker(
                                                            DAILY_BELIEF_REVISION_MARKER_KIND,
                                                            &date_str,
                                                        )
                                                    })
                                                    .await
                                                }
                                                .unwrap_or(false);

                                                if !marker_exists {
                                                    if let Err(e) =
                                                        br.run_pass(Utc::now().timestamp()).await
                                                    {
                                                        warn!(err.code = %e.code(), "belief revision pass failed: {e}");
                                                    } else {
                                                        let saved_marker = {
                                                            let sqlite6 = sqlite6.clone();
                                                            let date_str = date_str.clone();
                                                            offload_storage("daily belief marker save", move || {
                                                                sqlite6.save_digest_processing_marker(
                                                                    DAILY_BELIEF_REVISION_MARKER_KIND,
                                                                    &date_str,
                                                                    Utc::now(),
                                                                )
                                                            })
                                                            .await
                                                        };
                                                        if saved_marker.is_some() {
                                                            debug!("ADR-023: daily belief revision marked for {}", date_str);
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                }
                            }
                        }

                        // [HOUSEKEEPING] Vector index maintenance (count refresh, HNSW
                        // save, IVF rebuild, binary codes). This is just index-structure
                        // maintenance over existing vectors, not new-data collection, so
                        // it sits outside the gate. (Unlike re-embedding, it does not
                        // derive new embeddings.)
                        // --- Vector index maintenance (every 5 minutes) ---
                        if let Some(ref vi) = vector_index {
                            let should_run = last_index_maintenance
                                .map(|last| (now - last).num_minutes() >= 5)
                                .unwrap_or(true);

                            if should_run {
                                last_index_maintenance = Some(now);

                                // Refresh cached vector count in the search coordinator
                                if let Some(ref coord) = search_coordinator {
                                    if let Err(e) = coord.refresh_count().await {
                                        warn!("search coordinator refresh_count failure: {e}");
                                    }
                                }

                                // Periodic HNSW save (only writes if dirty)
                                #[cfg(feature = "hnsw")]
                                if let Some(ref ann) = ann_index {
                                    if let Err(e) = ann.save().await {
                                        warn!("HNSW periodic save failure: {e}");
                                    }
                                }

                                let embedding_config = config_manager
                                    .as_ref()
                                    .map(|cm| cm.get().analysis.embedding.clone())
                                    .unwrap_or_default();

                                if embedding_config.index_strategy != "brute_force" {
                                    match vi.get_index_meta().await {
                                        Ok(meta) => {
                                            let total = meta.total_vector_count;
                                            if total >= 10_000 {
                                                let needs_rebuild = meta.ivf_built_at.is_none()
                                                    || (meta.unindexed_count as f64 / total.max(1) as f64 > 0.10);

                                                if needs_rebuild {
                                                    let n_clusters = (total as f64).sqrt() as usize;
                                                    info!(
                                                        "Rebuilding IVF index: {} vectors, {} clusters",
                                                        total, n_clusters
                                                    );
                                                    if let Err(e) = vi.build_ivf_index(n_clusters, 10).await {
                                                        warn!("IVF index build failure: {e}");
                                                    }

                                                    if total > 100_000 {
                                                        info!("Building binary codes for {} vectors", total);
                                                        if let Err(e) = vi.build_binary_codes().await {
                                                            warn!("Binary code build failure: {e}");
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            warn!("get_index_meta failure: {e}");
                                        }
                                    }
                                }
                            }
                        }

                        // --- SQLite periodic maintenance (WAL checkpoint, FTS merge, conditional VACUUM) ---
                        // F-RR-06: all four calls acquire a parking_lot write lock — mirror the
                        // log_retention spawn_blocking pattern immediately below.
                        // #5809: await the handle so JoinError (including closure panics) is
                        // visible via warn!. Pre-#5640 these were inline-sequential; await
                        // restores that observability without blocking the reactor (the
                        // .await yields to the executor while the blocking thread runs).
                        {
                            let should_maintain = last_sqlite_maintenance
                                .map(|last| (now - last).num_minutes() >= super::super::config::SQLITE_MAINTENANCE_INTERVAL_MINS)
                                .unwrap_or(true);

                            if should_maintain {
                                last_sqlite_maintenance = Some(now);
                                let db = sqlite6.clone();
                                let fts_pages = super::super::config::FTS_MERGE_PAGES;
                                let vacuum_threshold = super::super::config::VACUUM_FREELIST_THRESHOLD_PERCENT;
                                let handle = tokio::task::spawn_blocking(move || {
                                    // WAL checkpoint (PASSIVE — non-blocking with respect to readers)
                                    if let Err(e) = db.wal_checkpoint_passive() {
                                        warn!("WAL checkpoint failure: {e}");
                                    }
                                    // FTS5 incremental merge
                                    if let Err(e) = db.fts_merge(fts_pages) {
                                        warn!("FTS5 merge failure: {e}");
                                    }
                                    // Conditional VACUUM (only when freelist > threshold)
                                    match db.maybe_vacuum(vacuum_threshold) {
                                        Ok(true) => info!("VACUUM completed during maintenance"),
                                        Ok(false) => {}
                                        Err(e) => warn!("VACUUM check failure: {e}"),
                                    }
                                });
                                if let Err(e) = handle.await {
                                    warn!("SQLite maintenance task join error: {e}");
                                }
                            }
                        }

                        // --- FTS5 daily full optimize ---
                        // F-RR-06: fts_optimize holds the parking_lot write lock for the full
                        // optimize pass (can be hundreds of ms) — use spawn_blocking.
                        // #5809: await so closure panics surface as JoinError warn logs.
                        {
                            let should_optimize = last_fts_optimize
                                .map(|last| (now - last).num_hours() >= 24)
                                .unwrap_or(true);

                            if should_optimize {
                                last_fts_optimize = Some(now);
                                let db = sqlite6.clone();
                                let handle = tokio::task::spawn_blocking(move || {
                                    if let Err(e) = db.fts_optimize() {
                                        warn!("FTS5 optimize failure: {e}");
                                    }
                                });
                                if let Err(e) = handle.await {
                                    warn!("FTS5 optimize task join error: {e}");
                                }
                            }
                        }

                        // --- Daily log file retention cleanup ---
                        // #5809: await so closure panics surface as JoinError warn logs
                        // (consistent with the two maintenance blocks above).
                        if let Some(ref dir) = log_dir {
                            let should_cleanup = last_log_cleanup
                                .map(|last| (now - last).num_hours() >= 24)
                                .unwrap_or(true);
                            if should_cleanup {
                                last_log_cleanup = Some(now);
                                let dir = dir.clone();
                                let handle = tokio::task::spawn_blocking(move || {
                                    crate::log_retention::cleanup_old_logs(
                                        &dir,
                                        crate::log_retention::DEFAULT_MAX_AGE_DAYS,
                                    );
                                });
                                if let Err(e) = handle.await {
                                    warn!("log retention task join error: {e}");
                                }
                            }
                        }

                        // --- Config file change detection ---
                        if let Some(ref cm) = config_manager {
                            check_config_file_changed(cm, &config_mtime).await;
                        }

                        debug!("completed");
                    }
                    // --- Regime state periodic crash-durability checkpoint (#5810/#7574) ---
                    // The shutdown path (main.rs RunEvent::Exit) is the authoritative save;
                    // this branch is a supplement that limits session loss on unclean exit
                    // to at most REGIME_CHECKPOINT_INTERVAL_MINS minutes. It runs on its own
                    // `regime_checkpoint_interval` timer (constructed above from the same
                    // constant) so the bound holds regardless of `aggregation_interval`.
                    //
                    // save_all calls conn.write_lock().run() synchronously (parking_lot
                    // mutex). The lock is held only for the duration of the SQLite execute
                    // calls (a few ms at most), so direct .await in this async context is
                    // acceptable — the blocking is bounded and infrequent (every 30 min). No
                    // spawn_blocking wrapper is added to keep the call site simple; this
                    // matches the main.rs shutdown pattern which also calls save_all
                    // directly inside a blocking runtime.
                    _ = regime_checkpoint_interval.tick() => {
                        if let (Some(ref rs), Some(ref rm)) =
                            (&regime_storage, &regime_manager_arc)
                        {
                            // Snapshot under the lock and release immediately — same
                            // pattern as main.rs:1017–1020.
                            let regimes = {
                                let guard = rm.lock();
                                guard.all_regimes().to_vec()
                            };
                            if !regimes.is_empty() {
                                if let Err(e) = rs.save_all(&regimes).await {
                                    warn!("regime checkpoint failure: {e}");
                                } else {
                                    debug!(count = regimes.len(), "regime checkpoint saved");
                                }
                            }
                        }
                    }
                    _ = shutdown_rx.changed() => {
                        info!("ended");
                        break;
                    }
                }
            }
        })
    }
}

/// Check if the config file has been modified on disk.
///
/// This is a free function (not a method) because the scheduler loops run
/// inside `tokio::spawn` blocks which clone individual fields — they do not
/// have access to `&self` (the Scheduler instance).
///
/// Uses `tokio::fs::metadata` so the async runtime thread is not blocked.
async fn check_config_file_changed(
    config_manager: &maekon_core::config_manager::ConfigManager,
    last_mtime: &parking_lot::Mutex<Option<std::time::SystemTime>>,
) -> bool {
    let path = config_manager.config_path();
    let Ok(metadata) = tokio::fs::metadata(path).await else {
        return false;
    };
    let Ok(modified) = metadata.modified() else {
        return false;
    };

    let mut prev = last_mtime.lock();
    match *prev {
        Some(prev_time) if modified > prev_time => {
            *prev = Some(modified);
            info!("config file changed — restart the application to apply new settings");
            true
        }
        None => {
            *prev = Some(modified);
            false
        }
        _ => false,
    }
}

/// ADR-023 (D3/D5): promote a freshly-built `DailyDigest` into durable
/// memory-graph claims + evidence edges via the [`MemoryGraphPort`].
///
/// Best-effort: a failed claim/edge write is logged (with the wire code) and
/// skipped — digest persistence already succeeded and a partial graph is
/// acceptable. Returns `true` only when all generated claim/edge writes completed,
/// so the caller can persist the per-date completion marker without hiding a
/// storage failure. Pure value construction lives in `maekon_analysis::claim_promoter`.
async fn persist_digest_memory_graph(
    memory_graph: &dyn maekon_core::ports::memory_graph_port::MemoryGraphPort,
    digest: &maekon_core::models::daily_digest::DailyDigest,
    now_secs: i64,
) -> bool {
    let mut claim_count = 0_usize;
    let mut failed = false;
    let pairs = maekon_analysis::claim_promoter::build_claims_from_digest(digest, now_secs);
    if pairs.is_empty() {
        return true;
    }
    for (claim, edges) in pairs {
        if let Err(e) = memory_graph.save_claim(&claim).await {
            failed = true;
            warn!(err.code = %e.code(), "memory-graph claim save failed: {e}");
            continue;
        }
        claim_count += 1;
        for edge in edges {
            if let Err(e) = memory_graph.add_edge(&edge).await {
                failed = true;
                warn!(err.code = %e.code(), "memory-graph evidence edge failed: {e}");
            }
        }
    }
    if claim_count > 0 {
        debug!("ADR-023: promoted {claim_count} digest claim(s) to the memory graph");
    }
    !failed
}

/// Offloads the scheduler's sync `SchedulerStorage` digest/segment calls to the
/// tokio blocking pool (#5097 / ADR-026 follow-up).
///
/// The digest/segment persistence methods go through `Arc<dyn SchedulerStorage>` (a
/// sync trait), so calling them directly from the aggregation loop would block the
/// async worker thread on SQLite I/O (other storage calls go through the async
/// `MetricsStorage` supertrait and are already non-blocking). Wrapping each call in
/// `spawn_blocking` moves the blocking to the blocking pool — the SQL/behavior is
/// identical to the sync method, and #4928 erase barrier's write_lock skip is
/// preserved as-is.
///
/// Failures (storage error or task panic) are logged with the `context` label and
/// `None` is returned, preserving the caller's best-effort semantics (the existing
/// `.ok()` / `if let Err`).
async fn offload_storage<T>(
    context: &'static str,
    job: impl FnOnce() -> Result<T, CoreError> + Send + 'static,
) -> Option<T>
where
    T: Send + 'static,
{
    match tokio::task::spawn_blocking(job).await {
        Ok(Ok(value)) => Some(value),
        Ok(Err(e)) => {
            warn!("{context} failure: {e}");
            None
        }
        Err(e) => {
            warn!("{context} task panicked: {e}");
            None
        }
    }
}

#[cfg(test)]
mod digest_catchup_tests {
    use super::{daily_catchup_dates, weekly_catchup_window_dates};
    use chrono::NaiveDate;
    use maekon_core::config::Weekday;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).expect("valid test date")
    }

    #[test]
    fn daily_catchup_runs_after_missed_midnight() {
        let dates = daily_catchup_dates(date(2026, 7, 1), Some(date(2026, 6, 29)), 90);

        assert_eq!(
            dates,
            vec![date(2026, 6, 29), date(2026, 6, 30)],
            "catch-up must not depend on still being in the local midnight hour"
        );
    }

    #[test]
    fn daily_catchup_includes_latest_digest_for_marker_repair() {
        let dates = daily_catchup_dates(date(2026, 7, 1), Some(date(2026, 6, 30)), 90);

        assert_eq!(
            dates,
            vec![date(2026, 6, 30)],
            "an existing digest still needs marker-gated claim/belief repair"
        );
    }

    #[test]
    fn weekly_catchup_finds_missed_digest_day_after_sleep() {
        let windows = weekly_catchup_window_dates(date(2026, 7, 6), Weekday::Sun, None, 52);

        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].start_date, date(2026, 6, 28));
        assert_eq!(windows[0].end_date, date(2026, 7, 5));
    }

    #[test]
    fn weekly_catchup_advances_from_latest_week_start() {
        let windows = weekly_catchup_window_dates(
            date(2026, 7, 6),
            Weekday::Sun,
            Some(date(2026, 6, 21)),
            52,
        );

        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].start_date, date(2026, 6, 28));
        assert_eq!(windows[0].end_date, date(2026, 7, 5));
    }
}

/// Unit tests for the `collection_permitted` helper.
///
/// Verifies only the gate decision logic without constructing the full Scheduler.
/// Covers six scenarios:
///   1. no config_manager → false (fail-closed)
///   2. no consent_manager (None) → false
///   3. consent not granted (NotGranted) → false
///   4. expired consent (Expired, screen_capture:true but expired) → false
///   5. valid consent (screen_capture=true) → true
///   6. valid consent but capture_paused → false
#[cfg(test)]
mod collection_permitted_tests {
    use super::{collection_permitted, embedding_reembedding_permitted};
    use maekon_core::config_manager::ConfigManager;
    use maekon_core::consent::{ConsentManager, ConsentPermissions};
    use maekon_core::ports::consent_manager::ConsentManagerPort;
    use std::sync::Arc;

    /// Returns a unique temporary file path for tests.
    /// Building the path from `std::env::temp_dir()` + a per-process monotonic
    /// counter (instead of `TempDir`) lets us write to the OS temp directory
    /// without deprecated-API warnings, while guaranteeing no collision between
    /// concurrently-running tests. A bare `subsec_nanos()` nonce collided under
    /// heavy parallel `--workspace` load — two tests picking the same temp name
    /// produced `ConfigManager::with_path` "File exists"/vanished-`.tmp` rename
    /// failures. The `process::id()` prefix keeps names unique across the several
    /// test binaries that share the OS temp dir; the atomic counter keeps them
    /// unique across threads within this binary.
    fn tmp_path(suffix: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let nonce = format!(
            "{}_{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        );
        std::env::temp_dir().join(format!("maekon_test_{nonce}_{suffix}"))
    }

    /// Helper that builds a ConfigManager from a temporary file path.
    /// `ConfigManager::with_path` creates defaults when the path does not exist.
    fn make_config_manager() -> ConfigManager {
        let path = tmp_path("config.json");
        ConfigManager::with_path(path).expect("ConfigManager creation failed")
    }

    /// Returns a ConsentManager that has granted valid screen_capture consent.
    fn make_valid_consent(screen_capture: bool) -> Arc<dyn ConsentManagerPort> {
        let consent_path = tmp_path("consent_valid.json");
        let mgr = Arc::new(ConsentManager::new(consent_path));
        let perms = ConsentPermissions {
            screen_capture,
            ..Default::default()
        };
        // Grant consent valid for 30 days
        mgr.grant_consent(perms, 30).expect("consent grant failed");
        mgr
    }

    fn make_valid_embedding_consent(
        screen_capture: bool,
        activity_pattern_learning: bool,
    ) -> Arc<dyn ConsentManagerPort> {
        let consent_path = tmp_path(&format!(
            "consent_embedding_{screen_capture}_{activity_pattern_learning}.json"
        ));
        let mgr = Arc::new(ConsentManager::new(consent_path));
        let perms = ConsentPermissions {
            screen_capture,
            activity_pattern_learning,
            ..Default::default()
        };
        mgr.grant_consent(perms, 30).expect("consent grant failed");
        mgr
    }

    /// Returns a ConsentManager in the not-granted state.
    fn make_no_consent_manager() -> Arc<dyn ConsentManagerPort> {
        let consent_path = tmp_path("consent_none.json");
        Arc::new(ConsentManager::new(consent_path))
    }

    /// Scenario 1: config_manager is None → fail-closed
    #[test]
    fn absent_config_manager_returns_false() {
        let consent = make_valid_consent(true);
        assert!(
            !collection_permitted(None, Some(&consent), false),
            "must always be false when config_manager is absent"
        );
    }

    /// Scenario 2: no consent (NotGranted) → false
    #[test]
    fn absent_consent_manager_returns_false() {
        let cm = make_config_manager();
        // consent_manager = None → effective_permissions is all-false
        assert!(
            !collection_permitted(Some(&cm), None, false),
            "must always be false when the consent manager is absent"
        );
    }

    /// Scenario 3: consent not granted (NotGranted) — a ConsentManager instance on
    /// which grant_consent has never been called has `check_consent() == NotGranted`,
    /// so `effective_permissions()` returns all-false and the gate must be closed.
    #[test]
    fn no_consent_granted_returns_false() {
        let cm = make_config_manager();
        let mgr = make_no_consent_manager(); // consent-not-granted state
        assert!(
            !collection_permitted(Some(&cm), Some(&mgr), false),
            "must always be false in the consent-not-granted state"
        );
    }

    /// Scenario 4: expired consent (Expired) — write a ConsentRecord with
    /// `screen_capture:true` but an `expires_at` in the past directly to a file,
    /// then load it via `ConsentManager::new` and verify that
    /// `effective_permissions()` returns all-false.
    /// Resuming collection on stale consent is the core risk of this task, so this
    /// must exist as a dedicated case.
    #[test]
    fn expired_consent_returns_false() {
        use maekon_core::consent::{ConsentRecord, CURRENT_POLICY_VERSION};
        // Write a ConsentRecord with a past expiry date to a file as JSON.
        let consent_path = tmp_path("consent_expired.json");
        let expired = ConsentRecord {
            consent_id: "exp-test".to_string(),
            version: CURRENT_POLICY_VERSION.to_string(),
            granted_at: chrono::Utc::now() - chrono::Duration::days(2),
            expires_at: Some(chrono::Utc::now() - chrono::Duration::days(1)),
            revoked_at: None,
            data_deletion_requested: false,
            erasure_nonce: None,
            permissions: ConsentPermissions {
                screen_capture: true, // the permission itself was granted but has expired
                ..Default::default()
            },
            data_retention_days: 30,
        };
        std::fs::write(
            &consent_path,
            serde_json::to_string(&expired).expect("serialization failed"),
        )
        .expect("file write failed");
        // ConsentManager::new reads the file and initializes into the Expired state.
        let mgr: Arc<dyn ConsentManagerPort> = Arc::new(ConsentManager::new(consent_path));
        let cm = make_config_manager();
        assert!(
            !collection_permitted(Some(&cm), Some(&mgr), false),
            "Expired consent must be fail-closed even with screen_capture:true"
        );
    }

    /// Scenario 5: valid consent (screen_capture=true) → true
    ///
    /// The AppConfig defaults have `vision.capture_enabled = true` and
    /// active_hours_enabled = false, so the active-hours gate passes.
    #[test]
    fn valid_consent_with_screen_capture_returns_true() {
        let cm = make_config_manager();
        let consent = make_valid_consent(true);
        assert!(
            collection_permitted(Some(&cm), Some(&consent), false),
            "must be true with valid consent and paused=false"
        );
    }

    /// Scenario 6: valid consent but capture_paused → false
    #[test]
    fn valid_consent_but_paused_returns_false() {
        let cm = make_config_manager();
        let consent = make_valid_consent(true);
        assert!(
            !collection_permitted(Some(&cm), Some(&consent), true),
            "must be false when paused=true regardless of consent"
        );
    }

    #[test]
    fn embedding_reembedding_requires_activity_pattern_learning_consent() {
        let cm = make_config_manager();
        let consent = make_valid_embedding_consent(true, false);
        assert!(
            collection_permitted(Some(&cm), Some(&consent), false),
            "baseline collection gate is open with valid screen_capture consent"
        );
        assert!(
            !embedding_reembedding_permitted(Some(&cm), Some(&consent), false),
            "remote embedding re-computation must remain closed without activity_pattern_learning consent"
        );
    }

    #[test]
    fn embedding_reembedding_runs_with_activity_pattern_learning_consent() {
        let cm = make_config_manager();
        let consent = make_valid_embedding_consent(true, true);
        assert!(
            embedding_reembedding_permitted(Some(&cm), Some(&consent), false),
            "remote embedding re-computation may run when both collection and activity_pattern_learning gates are open"
        );
    }
}

/// Unit tests for the `metrics_collection_permitted` helper (F1 / Option A).
///
/// The metrics loop handles infrastructure-health data (CONS-PM09 / spec §3.8 row
/// 16), so it is gated on consent (telemetry) alone and decoupled from
/// TS/pause/active-hours. This module pins down that it is intentionally a different
/// gate from process/aggregation's full-composite `collection_permitted`:
///   1. Valid consent + telemetry:true → true (no TrackingScheduleConfig input at all = TS-independent)
///   2. no consent_manager (None) → false (fail-closed)
///   3. consent not granted (NotGranted) → false
///   4. expired consent (Expired, telemetry:true but expired) → false (effective_permissions masks it)
///   5. policy version mismatch (UpdateRequired, telemetry:true) → false
///   6. Valid consent but telemetry:false → false (own-field gate)
#[cfg(test)]
mod metrics_collection_permitted_tests {
    use super::metrics_collection_permitted;
    use maekon_core::consent::{
        ConsentManager, ConsentPermissions, ConsentRecord, CURRENT_POLICY_VERSION,
    };
    use maekon_core::ports::consent_manager::ConsentManagerPort;
    use std::sync::Arc;

    /// Unique temporary file path for tests (per-process monotonic counter — see
    /// the collision note on `tmp_path` in `collection_permitted_tests`).
    fn tmp_path(suffix: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let nonce = format!(
            "{}_{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        );
        std::env::temp_dir().join(format!("maekon_metrics_test_{nonce}_{suffix}"))
    }

    /// A Valid ConsentManager that has been granted telemetry consent.
    fn make_valid_telemetry_consent() -> Arc<dyn ConsentManagerPort> {
        let mgr = Arc::new(ConsentManager::new(tmp_path("consent_telemetry.json")));
        mgr.grant_consent(
            ConsentPermissions {
                telemetry: true,
                ..Default::default()
            },
            30,
        )
        .expect("consent grant failed");
        mgr
    }

    /// Scenario 1: Valid consent + telemetry:true → true.
    ///
    /// This function takes neither config nor TS as arguments (its signature makes
    /// TS input impossible), so the gate opens with telemetry consent alone,
    /// regardless of whether TS is active = CONS-PM09 (metrics continue even during a
    /// TS mute). This is the key assertion proving that the metrics gate is decoupled
    /// from process/aggregation's full-composite gate.
    #[test]
    fn valid_telemetry_consent_returns_true_regardless_of_ts() {
        let consent = make_valid_telemetry_consent();
        assert!(
            metrics_collection_permitted(Some(&consent)),
            "valid telemetry consent must always be true regardless of TS/pause/active-hours (CONS-PM09)"
        );
    }

    /// Scenario 2: consent_manager None → fail-closed.
    #[test]
    fn absent_consent_manager_returns_false() {
        assert!(
            !metrics_collection_permitted(None),
            "must always be false when the consent manager is absent (fail-closed)"
        );
    }

    /// Scenario 3: consent not granted (NotGranted) → false.
    #[test]
    fn no_consent_granted_returns_false() {
        let mgr: Arc<dyn ConsentManagerPort> =
            Arc::new(ConsentManager::new(tmp_path("consent_none.json")));
        assert!(
            !metrics_collection_permitted(Some(&mgr)),
            "must always be false in the consent-not-granted state"
        );
    }

    /// Scenario 4: expired consent (Expired) — telemetry:true but expired → false.
    /// effective_permissions() masks any non-Valid state to all-false.
    #[test]
    fn expired_consent_returns_false() {
        let consent_path = tmp_path("consent_expired.json");
        let expired = ConsentRecord {
            consent_id: "exp-metrics".to_string(),
            version: CURRENT_POLICY_VERSION.to_string(),
            granted_at: chrono::Utc::now() - chrono::Duration::days(2),
            expires_at: Some(chrono::Utc::now() - chrono::Duration::days(1)),
            revoked_at: None,
            data_deletion_requested: false,
            erasure_nonce: None,
            permissions: ConsentPermissions {
                telemetry: true, // the permission itself was granted but has expired
                ..Default::default()
            },
            data_retention_days: 30,
        };
        std::fs::write(
            &consent_path,
            serde_json::to_string(&expired).expect("serialization failed"),
        )
        .expect("file write failed");
        let mgr: Arc<dyn ConsentManagerPort> = Arc::new(ConsentManager::new(consent_path));
        assert!(
            !metrics_collection_permitted(Some(&mgr)),
            "Expired consent must be fail-closed even with telemetry:true"
        );
    }

    /// Scenario 5: policy version mismatch (UpdateRequired) — telemetry:true but → false.
    /// Leaving expires_at=None forces the UpdateRequired branch rather than Expired.
    #[test]
    fn update_required_consent_returns_false() {
        let consent_path = tmp_path("consent_stale.json");
        let stale = ConsentRecord {
            consent_id: "stale-metrics".to_string(),
            version: "0.0.1".to_string(), // mismatched with the current policy version
            granted_at: chrono::Utc::now(),
            expires_at: None,
            revoked_at: None,
            data_deletion_requested: false,
            erasure_nonce: None,
            permissions: ConsentPermissions {
                telemetry: true,
                ..Default::default()
            },
            data_retention_days: 30,
        };
        std::fs::write(
            &consent_path,
            serde_json::to_string(&stale).expect("serialization failed"),
        )
        .expect("file write failed");
        let mgr: Arc<dyn ConsentManagerPort> = Arc::new(ConsentManager::new(consent_path));
        assert!(
            !metrics_collection_permitted(Some(&mgr)),
            "UpdateRequired consent must be fail-closed even with telemetry:true"
        );
    }

    /// Scenario 6: Valid consent but telemetry:false → false (own-field gate).
    /// Even with other permissions like screen_capture, metrics are blocked when
    /// telemetry is off.
    #[test]
    fn valid_consent_without_telemetry_returns_false() {
        let mgr: Arc<dyn ConsentManagerPort> =
            Arc::new(ConsentManager::new(tmp_path("consent_no_telemetry.json")));
        mgr.grant_consent(
            ConsentPermissions {
                screen_capture: true, // other permissions granted, but telemetry is false
                telemetry: false,
                ..Default::default()
            },
            30,
        )
        .expect("consent grant failed");
        assert!(
            !metrics_collection_permitted(Some(&mgr)),
            "the metrics gate must be closed when telemetry:false regardless of other permissions"
        );
    }
}

#[cfg(test)]
mod memory_graph_tests {
    use super::persist_digest_memory_graph;
    use chrono::Utc;
    use maekon_core::models::daily_digest::{DailyDigest, DailyStatistics, TimelineEntry};
    use maekon_core::models::memory_graph::{ClaimStatus, EdgeType};
    use maekon_core::ports::memory_graph_port::MemoryGraphPort;
    use maekon_storage::sqlite::SqliteStorage;

    fn digest_with_one_timeline_entry() -> DailyDigest {
        DailyDigest {
            date: chrono::NaiveDate::from_ymd_opt(2026, 5, 30).unwrap(),
            insight: None, // offline: no LLM insight
            timeline: vec![TimelineEntry {
                segment_id: "seg-x".to_string(),
                start_time: Utc::now(),
                end_time: Utc::now(),
                duration_mins: 60,
                regime_label: "Deep Focus".to_string(),
                regime_color: "#3B82F6".to_string(),
                dominant_app: "VS Code".to_string(),
                content_summary: vec![],
                annotation: None,
            }],
            statistics: DailyStatistics::default(),
            generated_at: Utc::now(),
        }
    }

    /// Exercises the production glue fn directly (its best-effort save loop), not a
    /// test re-implementation — closes the aggregation-loop coverage gap.
    #[tokio::test]
    async fn persist_digest_memory_graph_promotes_offline_claims() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        persist_digest_memory_graph(&storage, &digest_with_one_timeline_entry(), 1_700_000_000)
            .await;

        let active = storage
            .list_claims_by_status(ClaimStatus::Active)
            .await
            .unwrap();
        assert_eq!(
            active.len(),
            1,
            "offline digest promotes one timeline claim"
        );
        assert_eq!(active[0].source, "digest_timeline");

        let edges = storage
            .edges_from(&active[0].claim_id, Some(EdgeType::Evidence))
            .await
            .unwrap();
        assert_eq!(edges.len(), 1, "the timeline claim has an evidence edge");
        assert_eq!(edges[0].dst_id, "seg-x");
    }
}

/// Contract tests for `spawn_aggregation_loop`'s collect/derive gate.
///
/// The aggregation loop places its data derivation/persistence blocks (hourly
/// aggregation, daily/weekly digests, memory-graph claims, stale vector
/// re-embedding) behind `collect_ok`, and `collect_ok` is computed by R4's
/// `collection_permitted` free fn (= the same gate as the metrics/process loops).
/// Since `collection_permitted_tests` already covers all six scenarios (no config /
/// no consent / not granted / expired / valid / paused), here we pin down only the
/// security-critical properties of the *aggregation context*:
///   - no consent (not granted) → collect_ok=false → skip digest/claim/re-embed writes
///   - valid consent → collect_ok=true → run derive writes
///
/// Housekeeping (retention deletes, sqlite maintenance, index maintenance) always
/// runs regardless of the gate; this is guaranteed by the code structure (placed
/// outside the gate) + compilation.
///
/// Note: an integration test that constructs the full Scheduler + 16 ports + the
/// tokio runtime + a midnight trigger to spy on the actual writes would be
/// excessive. The gate decision is extracted into a pure helper and is thus
/// sufficiently verified by unit tests (the should_rearm_vad pattern).
#[cfg(test)]
mod aggregation_gate_tests {
    use super::collection_permitted;
    use maekon_core::config_manager::ConfigManager;
    use maekon_core::consent::{ConsentManager, ConsentPermissions};
    use maekon_core::ports::consent_manager::ConsentManagerPort;
    use std::sync::Arc;

    /// Unique temporary file path for tests (per-process monotonic counter — see
    /// the collision note on `tmp_path` in `collection_permitted_tests`).
    fn tmp_path(suffix: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let nonce = format!(
            "{}_{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        );
        std::env::temp_dir().join(format!("maekon_agg_test_{nonce}_{suffix}"))
    }

    fn make_config_manager() -> ConfigManager {
        ConfigManager::with_path(tmp_path("config.json")).expect("ConfigManager creation failed")
    }

    /// Consent-not-granted state → the aggregation loop's collect/derive gate
    /// (collect_ok) must be closed. That is, daily/weekly digest, memory-graph claim,
    /// and re-embedding writes are skipped.
    #[test]
    fn aggregation_derive_writes_skipped_without_consent() {
        let cm = make_config_manager();
        let no_consent: Arc<dyn ConsentManagerPort> =
            Arc::new(ConsentManager::new(tmp_path("consent_none.json")));
        let collect_ok = collection_permitted(Some(&cm), Some(&no_consent), false);
        assert!(
            !collect_ok,
            "when consent is not granted, the aggregation loop's derive/persist blocks must not run (gate closed)"
        );
    }

    /// Valid consent (screen_capture=true, paused=false) → collect_ok=true → run
    /// derive writes.
    #[test]
    fn aggregation_derive_writes_run_with_valid_consent() {
        let cm = make_config_manager();
        let mgr: Arc<dyn ConsentManagerPort> =
            Arc::new(ConsentManager::new(tmp_path("consent_valid.json")));
        mgr.grant_consent(
            ConsentPermissions {
                screen_capture: true,
                ..Default::default()
            },
            30,
        )
        .expect("consent grant failed");
        let collect_ok = collection_permitted(Some(&cm), Some(&mgr), false);
        assert!(
            collect_ok,
            "with valid consent + paused=false, the aggregation loop's derive blocks must run"
        );
    }

    /// When paused=true, collect_ok=false even with valid consent (derive writes
    /// skipped) — housekeeping continues independently (guaranteed by code structure).
    #[test]
    fn aggregation_derive_writes_skipped_when_paused() {
        let cm = make_config_manager();
        let mgr: Arc<dyn ConsentManagerPort> =
            Arc::new(ConsentManager::new(tmp_path("consent_paused.json")));
        mgr.grant_consent(
            ConsentPermissions {
                screen_capture: true,
                ..Default::default()
            },
            30,
        )
        .expect("consent grant failed");
        assert!(
            !collection_permitted(Some(&cm), Some(&mgr), true),
            "when paused=true the derive-block gate must be closed (housekeeping continues outside the gate)"
        );
    }
}

#[cfg(test)]
mod stale_vector_reembed_tests {
    use super::reembed_stale_vectors;
    use async_trait::async_trait;
    use maekon_core::error::CoreError;
    use maekon_core::error_codes::InternalCode;
    use maekon_core::models::embedding::{EmbeddingMetadata, SearchFilters, SearchResult};
    use maekon_core::ports::embedding_provider::EmbeddingProvider;
    use maekon_core::ports::vector_store::VectorStore;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FixedEmbeddingProvider;

    #[async_trait]
    impl EmbeddingProvider for FixedEmbeddingProvider {
        async fn embed(&self, _text: &str) -> Result<Vec<f32>, CoreError> {
            Ok(vec![1.0])
        }

        async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, CoreError> {
            Ok(texts.iter().map(|_| vec![1.0]).collect())
        }

        fn dimensions(&self) -> usize {
            1
        }

        fn model_id(&self) -> &str {
            "test-model"
        }
    }

    struct WriteFaultVectorStore {
        get_calls: AtomicUsize,
        update_calls: AtomicUsize,
    }

    impl WriteFaultVectorStore {
        fn new() -> Self {
            Self {
                get_calls: AtomicUsize::new(0),
                update_calls: AtomicUsize::new(0),
            }
        }
    }

    struct ZeroRowVectorStore {
        get_calls: AtomicUsize,
        update_calls: AtomicUsize,
    }

    impl ZeroRowVectorStore {
        fn new() -> Self {
            Self {
                get_calls: AtomicUsize::new(0),
                update_calls: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl VectorStore for WriteFaultVectorStore {
        async fn store(
            &self,
            _vector: Vec<f32>,
            _metadata: EmbeddingMetadata,
        ) -> Result<(), CoreError> {
            Ok(())
        }

        async fn search(
            &self,
            _query_vector: &[f32],
            _limit: usize,
            _time_decay_hours: f32,
        ) -> Result<Vec<SearchResult>, CoreError> {
            Ok(Vec::new())
        }

        async fn search_filtered(
            &self,
            _query_vector: &[f32],
            _limit: usize,
            _time_decay_hours: f32,
            _filters: &SearchFilters,
        ) -> Result<Vec<SearchResult>, CoreError> {
            Ok(Vec::new())
        }

        async fn enforce_retention(&self, _max_days: u32) -> Result<u64, CoreError> {
            Ok(0)
        }

        async fn mark_stale(&self, _old_model_id: &str) -> Result<u64, CoreError> {
            Ok(0)
        }

        async fn update_vector(
            &self,
            _id: i64,
            _vector: Vec<f32>,
            _model_id: &str,
        ) -> Result<u64, CoreError> {
            self.update_calls.fetch_add(1, Ordering::SeqCst);
            Err(CoreError::Internal {
                code: InternalCode::Generic,
                message: "synthetic write fault".to_string(),
            })
        }

        async fn get_current_model_id(&self) -> Result<Option<String>, CoreError> {
            Ok(Some("old-model".to_string()))
        }

        async fn get_stale_vectors(&self, _limit: usize) -> Result<Vec<(i64, String)>, CoreError> {
            self.get_calls.fetch_add(1, Ordering::SeqCst);
            Ok(vec![(1, "alpha".to_string()), (2, "beta".to_string())])
        }
    }

    #[async_trait]
    impl VectorStore for ZeroRowVectorStore {
        async fn store(
            &self,
            _vector: Vec<f32>,
            _metadata: EmbeddingMetadata,
        ) -> Result<(), CoreError> {
            Ok(())
        }

        async fn search(
            &self,
            _query_vector: &[f32],
            _limit: usize,
            _time_decay_hours: f32,
        ) -> Result<Vec<SearchResult>, CoreError> {
            Ok(Vec::new())
        }

        async fn search_filtered(
            &self,
            _query_vector: &[f32],
            _limit: usize,
            _time_decay_hours: f32,
            _filters: &SearchFilters,
        ) -> Result<Vec<SearchResult>, CoreError> {
            Ok(Vec::new())
        }

        async fn enforce_retention(&self, _max_days: u32) -> Result<u64, CoreError> {
            Ok(0)
        }

        async fn mark_stale(&self, _old_model_id: &str) -> Result<u64, CoreError> {
            Ok(0)
        }

        async fn update_vector(
            &self,
            _id: i64,
            _vector: Vec<f32>,
            _model_id: &str,
        ) -> Result<u64, CoreError> {
            self.update_calls.fetch_add(1, Ordering::SeqCst);
            Ok(0)
        }

        async fn get_current_model_id(&self) -> Result<Option<String>, CoreError> {
            Ok(Some("old-model".to_string()))
        }

        async fn get_stale_vectors(&self, _limit: usize) -> Result<Vec<(i64, String)>, CoreError> {
            self.get_calls.fetch_add(1, Ordering::SeqCst);
            Ok(vec![(1, "alpha".to_string()), (2, "beta".to_string())])
        }
    }

    #[tokio::test]
    async fn stale_vector_reembed_breaks_when_batch_makes_no_progress() {
        let store = WriteFaultVectorStore::new();
        let provider = FixedEmbeddingProvider;

        let stats = reembed_stale_vectors(&store, &provider, 2, 10).await;

        assert_eq!(store.get_calls.load(Ordering::SeqCst), 1);
        assert_eq!(store.update_calls.load(Ordering::SeqCst), 2);
        assert_eq!(stats.batches, 1);
        assert_eq!(stats.updated, 0);
        assert_eq!(stats.failed, 2);
        assert!(!stats.hit_batch_cap);
    }

    #[tokio::test]
    async fn stale_vector_reembed_stops_at_batch_cap_when_zero_row_updates_repeat() {
        let store = ZeroRowVectorStore::new();
        let provider = FixedEmbeddingProvider;

        let stats = reembed_stale_vectors(&store, &provider, 2, 3).await;

        assert_eq!(store.get_calls.load(Ordering::SeqCst), 3);
        assert_eq!(store.update_calls.load(Ordering::SeqCst), 6);
        assert_eq!(stats.batches, 3);
        assert_eq!(stats.missing_or_deleted, 6);
        assert!(stats.hit_batch_cap);
    }
}

/// #5809: spawn_blocking JoinError visibility contract tests.
///
/// The three maintenance blocks (SQLite maintenance, FTS optimize, log retention)
/// all await their spawn_blocking handles. This module verifies that the tokio
/// task-isolation + JoinError observation contract holds — i.e. a panic in the
/// spawn_blocking closure produces a JoinError that callers CAN inspect via .await.
/// (The production loop logs it with warn!; here we assert the observable Err.)
#[cfg(test)]
mod spawn_blocking_join_error_tests {
    /// A spawn_blocking closure that panics produces JoinError::is_panic() == true
    /// when its handle is awaited — verifying that the maintenance blocks are now
    /// observable rather than silent fire-and-forget.
    #[tokio::test]
    async fn awaited_spawn_blocking_panic_surfaces_as_join_error() {
        let handle = tokio::task::spawn_blocking(|| {
            panic!("intentional maintenance closure panic");
        });
        let result = handle.await;
        let err = result.expect_err("panicking spawn_blocking must return Err on await");
        assert!(
            err.is_panic(),
            "JoinError from a panicking spawn_blocking must be is_panic(): {err:?}"
        );
    }

    /// A spawn_blocking closure that completes normally returns Ok(()) when awaited —
    /// confirming that non-panicking maintenance still succeeds (no regression).
    #[tokio::test]
    async fn awaited_spawn_blocking_success_returns_ok() {
        let handle = tokio::task::spawn_blocking(|| {
            // Normal maintenance — no panic.
        });
        handle
            .await
            .expect("non-panicking spawn_blocking must return Ok when awaited");
    }
}

/// #5810/#7582: regime checkpoint interval sanity test.
///
/// Before #7582, the crash-durability checkpoint was gated by an inline
/// `last_regime_checkpoint.map(|last| (now - last).num_minutes() >=
/// REGIME_CHECKPOINT_INTERVAL_MINS).unwrap_or(true)` check nested inside the
/// aggregation-interval tick, and this module mirrored that same expression
/// (without calling into production code) to pin three cases: first-call,
/// within-window, and after-window.
///
/// #7582 moved the checkpoint onto its own independent `regime_checkpoint_interval`
/// sub-tick timer (see `spawn_aggregation_loop` above) — there is no more
/// `last`/`num_minutes()` gate expression to mirror, so the three gate-shaped
/// cases were tautological (a copy of a deleted computation, not a call into
/// production code) and have been removed. The real fire-cadence behavior —
/// the first tick checkpoints immediately, and a later checkpoint fires
/// independent of a long aggregation interval — is covered end-to-end (via the
/// real `spawn_aggregation_loop`) by
/// `network::tests::regime_checkpoint_fires_independent_of_long_aggregation_interval`.
///
/// This module keeps the one check with independent regression value: the
/// interval constant itself must stay positive. A `<= 0` value would otherwise
/// surface only as a `tokio::time::interval` panic ("period must be non-zero")
/// buried inside that async, paused-clock test.
#[cfg(test)]
mod regime_checkpoint_interval_tests {
    /// Interval constant must exist and be positive (compile-time existence check
    /// + runtime sanity guard — value is not hard-coded in production code).
    #[test]
    fn regime_checkpoint_interval_constant_is_positive() {
        let mins = super::super::super::config::REGIME_CHECKPOINT_INTERVAL_MINS;
        assert!(
            mins > 0,
            "REGIME_CHECKPOINT_INTERVAL_MINS must be > 0, got {mins}"
        );
    }
}
