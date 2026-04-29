use chrono::{Datelike, Duration as ChronoDuration, Timelike, Utc};
use oneshim_core::models::activity::{ProcessSnapshot, ProcessSnapshotEntry};
use oneshim_web::{MetricsUpdate, RealtimeEvent};
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};

use super::super::Scheduler;
use super::helpers::record_to_segment_summary;

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

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(metrics_interval);

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        match sys_mon.collect_metrics().await {
                            Ok(metrics) => {
                                if let Err(e) = sqlite2.save_metrics(&metrics).await {
                                    warn!("system save failure: {e}");
                                }

                                let memory_percent = if metrics.memory_total > 0 {
                                    (metrics.memory_used as f32 / metrics.memory_total as f32) * 100.0
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
                                    notif.check_high_usage(metrics.cpu_usage, memory_percent).await;
                                }
                            }
                            Err(e) => {
                                warn!("system collect failure: {e}");
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

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(process_interval);

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        match proc_mon.get_top_processes(10).await {
                            Ok(processes) => {
                                let snapshot = ProcessSnapshot {
                                    timestamp: Utc::now(),
                                    processes: processes.into_iter().map(|p| ProcessSnapshotEntry {
                                        pid: p.pid,
                                        name: p.name,
                                        cpu_usage: p.cpu_usage,
                                        memory_bytes: p.memory_bytes,
                                    }).collect(),
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
        llm_summarizer: Option<Arc<oneshim_analysis::LlmSegmentSummarizer>>,
        mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) -> tokio::task::JoinHandle<()> {
        let sqlite6 = self.sqlite_storage.clone();
        let vector_store = self.vector_store.clone();
        let embedding_provider = self.embedding_provider.clone();
        let config_manager = self.config_manager.clone();
        let vector_index = self.vector_index.clone();
        let search_coordinator = self.search_coordinator.clone();
        #[cfg(feature = "hnsw")]
        let ann_index = self.ann_index.clone();

        // Resolve log directory once for periodic log retention cleanup.
        let log_dir = oneshim_core::config_manager::ConfigManager::data_dir()
            .map(|d| d.join("logs"))
            .ok();

        // Config file mtime tracker — shared into the spawned task.
        let config_mtime: Arc<parking_lot::Mutex<Option<std::time::SystemTime>>> =
            Arc::new(parking_lot::Mutex::new(None));

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(aggregation_interval);
            let mut last_reindex_check: Option<chrono::DateTime<Utc>> = None;
            let mut last_index_maintenance: Option<chrono::DateTime<Utc>> = None;
            let mut last_log_cleanup: Option<chrono::DateTime<Utc>> = None;
            let mut last_sqlite_maintenance: Option<chrono::DateTime<Utc>> = None;
            let mut last_fts_optimize: Option<chrono::DateTime<Utc>> = None;

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        let now = Utc::now();

                        let prev_hour = now - ChronoDuration::hours(1);
                        if let Err(e) = sqlite6.aggregate_hourly_metrics(prev_hour).await {
                            warn!("hour failure: {e}");
                        }

                        let metrics_cutoff = now - ChronoDuration::hours(super::super::config::RAW_METRICS_RETENTION_HOURS);
                        if let Err(e) = sqlite6.cleanup_old_metrics(metrics_cutoff).await {
                            warn!("delete failure: {e}");
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

                                // Process stale vectors in batches of 100
                                loop {
                                    match vs.get_stale_vectors(100).await {
                                        Ok(batch) if !batch.is_empty() => {
                                            let texts: Vec<String> = batch.iter().map(|(_, t)| t.clone()).collect();
                                            match ep.embed_batch(&texts).await {
                                                Ok(vectors) => {
                                                    let model_id = ep.model_id();
                                                    let mut updated = 0u64;
                                                    for ((id, _), vec) in batch.into_iter().zip(vectors) {
                                                        if let Err(e) = vs.update_vector(id, vec, model_id).await {
                                                            warn!("re-embed update failure: {e}");
                                                        } else {
                                                            updated += 1;
                                                        }
                                                    }
                                                    debug!("re-embedded {updated} stale vectors");
                                                }
                                                Err(e) => {
                                                    warn!("re-embed batch failure: {e}");
                                                    break;
                                                }
                                            }
                                        }
                                        Ok(_) => break, // no more stale vectors
                                        Err(e) => {
                                            warn!("get stale vectors failure: {e}");
                                            break;
                                        }
                                    }
                                }

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

                        // --- Activity segment retention (default: 90 days, same as embedding) ---
                        {
                            let segment_retention_days = config_manager
                                .as_ref()
                                .map(|cm| cm.get().analysis.embedding.retention_days)
                                .unwrap_or(90);
                            if let Err(e) = sqlite6.enforce_segment_retention(segment_retention_days) {
                                warn!("segment retention failure: {e}");
                            }

                            // Weekly digests retention (keep 52 weeks = 1 year)
                            if let Err(e) = sqlite6.enforce_digest_retention(52) {
                                warn!("digest retention failure: {e}");
                            }

                            // Auxiliary table retention (work_sessions, interruptions, etc.)
                            if let Err(e) = sqlite6.enforce_all_retention() {
                                warn!("auxiliary table retention failure: {e}");
                            }
                        }

                        // --- Weekly digest auto-generation ---
                        {
                            let digest_day = config_manager
                                .as_ref()
                                .map(|cm| cm.get().analysis.embedding.digest_day)
                                .unwrap_or(oneshim_core::config::Weekday::Sun);

                            let local_now = chrono::Local::now();
                            let is_digest_day =
                                local_now.weekday().num_days_from_sunday() == digest_day.num_days_from_sunday();
                            let is_midnight_hour = local_now.hour() == 0;

                            if is_digest_day && is_midnight_hour {
                                // Calculate week boundaries (Monday-based ISO week aligned to digest_day)
                                let week_end = now;
                                let week_start = now - ChronoDuration::days(7);

                                // Check if digest already exists for this week
                                let existing = sqlite6
                                    .list_weekly_digests(1)
                                    .ok()
                                    .and_then(|d| d.into_iter().next());

                                let already_generated = existing
                                    .as_ref()
                                    .map(|d| (now - d.week_end).num_hours() < 24)
                                    .unwrap_or(false);

                                if !already_generated {
                                    // Load actual segments for this week from storage
                                    let week_segments = sqlite6
                                        .list_segments_between(week_start, week_end)
                                        .unwrap_or_default();
                                    let digest = oneshim_analysis::WeeklyDigestGenerator::generate(
                                        &week_segments,
                                        week_start,
                                        week_end,
                                        existing.as_ref(),
                                    );

                                    if let Err(e) = sqlite6.save_weekly_digest(&digest) {
                                        warn!("weekly digest save failure: {e}");
                                    } else {
                                        info!("Weekly digest generated for week ending {}", week_end);
                                    }
                                }
                            }
                        }

                        // --- Daily digest auto-generation (midnight) ---
                        {
                            let local_now = chrono::Local::now();
                            if local_now.hour() == 0 {
                                // Generate digest for yesterday
                                let yesterday = local_now.date_naive()
                                    .pred_opt()
                                    .unwrap_or(local_now.date_naive());
                                let date_str = yesterday.format("%Y-%m-%d").to_string();

                                // Check if daily digest already exists
                                let existing = sqlite6
                                    .get_daily_digest(&date_str)
                                    .ok()
                                    .flatten();

                                if existing.is_none() {
                                    // Load segments for yesterday
                                    let segment_records = sqlite6
                                        .get_segments_for_date(&date_str)
                                        .unwrap_or_default();

                                    if !segment_records.is_empty() {
                                        // Convert SegmentSummaryRecords to SegmentSummary for DailyDigestGenerator
                                        let segments: Vec<oneshim_core::models::tiered_memory::SegmentSummary> =
                                            segment_records
                                                .iter()
                                                .filter_map(record_to_segment_summary)
                                                .collect();

                                        // Load previous day for comparison
                                        let prev_date = yesterday
                                            .pred_opt()
                                            .unwrap_or(yesterday)
                                            .format("%Y-%m-%d")
                                            .to_string();
                                        let prev_digest = sqlite6
                                            .get_daily_digest(&prev_date)
                                            .ok()
                                            .flatten();

                                        let mut digest = oneshim_analysis::DailyDigestGenerator::generate(
                                            &segments,
                                            yesterday,
                                            prev_digest.as_ref(),
                                        );

                                        // Generate LLM narrative insight if provider is available.
                                        if let Some(ref summarizer) = llm_summarizer {
                                            let pii_level = config_manager
                                                .as_ref()
                                                .map(|cm| cm.get().privacy.pii_filter_level)
                                                .unwrap_or(oneshim_core::config::PiiFilterLevel::Standard);
                                            let pii_filter: oneshim_analysis::PiiFilter =
                                                Box::new(move |text: &str| {
                                                    oneshim_vision::privacy::sanitize_title_with_level(text, pii_level)
                                                });
                                            let insight_gen = oneshim_analysis::DailyInsightGenerator::new(
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

                                        if let Err(e) = sqlite6.save_daily_digest(&digest) {
                                            warn!("daily digest save failure: {e}");
                                        } else {
                                            info!("Daily digest generated for {}", date_str);
                                        }
                                    }
                                }
                            }
                        }

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
                        {
                            let should_maintain = last_sqlite_maintenance
                                .map(|last| (now - last).num_minutes() >= super::super::config::SQLITE_MAINTENANCE_INTERVAL_MINS)
                                .unwrap_or(true);

                            if should_maintain {
                                last_sqlite_maintenance = Some(now);

                                // WAL checkpoint (PASSIVE — non-blocking)
                                if let Err(e) = sqlite6.wal_checkpoint_passive() {
                                    warn!("WAL checkpoint failure: {e}");
                                }

                                // FTS5 incremental merge
                                if let Err(e) = sqlite6.fts_merge(super::super::config::FTS_MERGE_PAGES) {
                                    warn!("FTS5 merge failure: {e}");
                                }

                                // Conditional VACUUM (only when freelist > 20%)
                                match sqlite6.maybe_vacuum(super::super::config::VACUUM_FREELIST_THRESHOLD_PERCENT) {
                                    Ok(true) => info!("VACUUM completed during maintenance"),
                                    Ok(false) => {}
                                    Err(e) => warn!("VACUUM check failure: {e}"),
                                }
                            }
                        }

                        // --- FTS5 daily full optimize ---
                        {
                            let should_optimize = last_fts_optimize
                                .map(|last| (now - last).num_hours() >= 24)
                                .unwrap_or(true);

                            if should_optimize {
                                last_fts_optimize = Some(now);
                                if let Err(e) = sqlite6.fts_optimize() {
                                    warn!("FTS5 optimize failure: {e}");
                                }
                            }
                        }

                        // --- Daily log file retention cleanup ---
                        if let Some(ref dir) = log_dir {
                            let should_cleanup = last_log_cleanup
                                .map(|last| (now - last).num_hours() >= 24)
                                .unwrap_or(true);
                            if should_cleanup {
                                last_log_cleanup = Some(now);
                                let dir = dir.clone();
                                tokio::task::spawn_blocking(move || {
                                    crate::log_retention::cleanup_old_logs(
                                        &dir,
                                        crate::log_retention::DEFAULT_MAX_AGE_DAYS,
                                    );
                                });
                            }
                        }

                        // --- Config file change detection ---
                        if let Some(ref cm) = config_manager {
                            check_config_file_changed(cm, &config_mtime);
                        }

                        debug!("completed");
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
fn check_config_file_changed(
    config_manager: &oneshim_core::config_manager::ConfigManager,
    last_mtime: &parking_lot::Mutex<Option<std::time::SystemTime>>,
) -> bool {
    let path = config_manager.config_path();
    let Ok(metadata) = std::fs::metadata(path) else {
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
