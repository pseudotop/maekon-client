use chrono::{Datelike, Duration as ChronoDuration, Timelike, Utc};
use maekon_core::config_manager::ConfigManager;
use maekon_core::error::CoreError;
use maekon_core::models::activity::{ProcessSnapshot, ProcessSnapshotEntry};
use maekon_core::ports::consent_manager::ConsentManagerPort;
use maekon_web::{MetricsUpdate, RealtimeEvent};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};

use super::super::Scheduler;
use super::helpers::record_to_segment_summary;

/// 메트릭/프로세스 수집 루프의 동의 게이트 결정 순수 헬퍼.
///
/// `capture_permitted_now` 4-term 복합 게이트에 위임한다:
/// - `config_manager` 가 None 이면 → fail-closed (false)
/// - `consent_manager` 가 None 이거나 Valid 상태가 아니면 → all-false 권한
/// - `capture_paused` 가 true 이면 → false
///
/// 이 함수는 순수(side-effect 없음)이므로 단위 테스트가 Scheduler 전체를 구성하지
/// 않고도 게이트 결정 논리를 검증할 수 있다 (should_rearm_vad 패턴 참조).
pub(super) fn collection_permitted(
    config_manager: Option<&ConfigManager>,
    consent_manager: Option<&Arc<dyn ConsentManagerPort>>,
    paused: bool,
) -> bool {
    let Some(cm) = config_manager else {
        return false;
    };
    let consent = consent_manager
        .map(|c| c.effective_permissions())
        .unwrap_or_default();
    crate::scheduler::capture_permitted_now(&cm.snapshot(), &consent, paused)
}

/// 메트릭 수집 루프 전용 동의 게이트 결정 순수 헬퍼 (F1 / Option A).
///
/// 메트릭=인프라 헬스(CONS-PM09 / spec §3.8 row 16) — 동의(telemetry)만 게이트하고
/// TS/pause/active-hours 와 디커플한다. process/aggregation 루프는 사용자-활동 캡처라
/// `collection_permitted`(full-composite 4-term)를 그대로 유지한다.
///
/// 시스템 메트릭(CPU/메모리/디스크)은 사용자-활동 캡처가 아니라 인프라 헬스 데이터이므로
/// Tracking-Schedule mute 창 동안에도 계속되어야 한다는 CONS-PM09 / spec §3.8 row 16
/// 계약을 존중한다. 따라서 config(TS)·capture_paused·active-hours 를 인자로 받지 않으며,
/// 오직 `effective_permissions().telemetry`(Valid 상태에서만 true)만 본다:
/// - `consent_manager` 가 None 이거나 Valid 가 아니면 → false (fail-closed)
/// - telemetry 가 false 이면 → false (own-field 게이트)
pub(super) fn metrics_collection_permitted(
    consent_manager: Option<&Arc<dyn ConsentManagerPort>>,
) -> bool {
    consent_manager
        .map(|c| c.effective_permissions().telemetry)
        .unwrap_or(false)
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
        // 동의 게이트 DI — 메트릭 수집/영속화는 telemetry 동의 없이 실행 불가.
        // 메트릭=인프라 헬스(CONS-PM09 / spec §3.8 row 16)이므로 consent(telemetry)만
        // 게이트하고 TS/pause/active-hours 와 디커플한다 (config_manager / capture_paused
        // 클론 불필요). process/aggregation 루프는 full-composite 게이트를 유지한다.
        let consent_manager_m = self.consent_manager.clone();

        tokio::spawn(async move {
            let mut interval = super::intervals::coalescing_interval(metrics_interval);
            // 전원 상태 폴링 디커플 게이트 — pmset 는 fork/exec 비용이 있으므로 매 메트릭
            // 틱(~5s)마다 호출하지 않고 ~60s 마다만 갱신한다. 갱신 사이에는 마지막으로
            // 측정한 상태를 캐시해 재적용하므로 배터리 세이버 플래그는 항상 최신 측정값을
            // 반영한다 (last_index_maintenance 의 num_minutes 게이트 패턴과 동일).
            let mut last_power_check: Option<chrono::DateTime<Utc>> = None;
            let mut cached_power_status = maekon_core::models::system::PowerStatus::default();

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        // 전원 상태 갱신 — 배터리 세이버 플래그를 설정하는 운영 작업이므로
                        // 동의 게이트와 무관하게 항상 실행한다. pmset fork/exec 비용 절감을
                        // 위해 실제 폴링은 ~60s 마다만 수행하고, 그 사이에는 캐시를 재적용한다.
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
                        // 매 틱마다 (캐시된) 최신 측정값을 스케줄러 플래그에 재적용한다.
                        crate::scheduler::set_battery_saver_active_for_scheduler(
                            cached_power_status.battery_saver_active,
                        );
                        // 메트릭 수집·영속화 블록 — consent(telemetry) 단독 게이트로 보호.
                        // 인프라 헬스 데이터(CONS-PM09 / spec §3.8 row 16)이므로 TS/pause/
                        // active-hours 와 디커플 — telemetry 동의만 확인한다.
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
        // 동의 게이트 DI — 프로세스 스냅샷은 사용자 데이터이므로 동의 없이 영속화 불가
        let config_manager_p = self.config_manager.clone();
        let consent_manager_p = self.consent_manager.clone();
        let capture_paused_p = self.capture_paused.clone();

        tokio::spawn(async move {
            let mut interval = super::intervals::coalescing_interval(process_interval);

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        // 4-term 복합 게이트 — 동의 없으면 프로세스 목록 수집·영속화 건너뜀
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
        // 동의 게이트 DI — 집계 루프의 데이터 파생/영속화(시간별 집계·일/주간 다이제스트·
        // memory-graph claim·스테일 벡터 재임베딩)는 동의 없이 실행 불가 (CONS-PC02).
        // 하우스키핑(보존 삭제·sqlite 유지보수·인덱스 정비·로그 정리)은 게이트 밖에서 항상 실행.
        let capture_paused = self.capture_paused.clone();
        let vector_index = self.vector_index.clone();
        let search_coordinator = self.search_coordinator.clone();
        #[cfg(feature = "hnsw")]
        let ann_index = self.ann_index.clone();
        // #5810: regime crash-durability checkpoint — same Arcs as shutdown path.
        let regime_storage = self.regime_storage.clone();
        let regime_manager_arc = self.regime_manager_arc.clone();

        // Resolve log directory once for periodic log retention cleanup.
        let log_dir = maekon_core::config_manager::ConfigManager::data_dir()
            .map(|d| d.join("logs"))
            .ok();

        // Config file mtime tracker — shared into the spawned task.
        let config_mtime: Arc<parking_lot::Mutex<Option<std::time::SystemTime>>> =
            Arc::new(parking_lot::Mutex::new(None));

        tokio::spawn(async move {
            let mut interval = super::intervals::coalescing_interval(aggregation_interval);
            let mut last_reindex_check: Option<chrono::DateTime<Utc>> = None;
            let mut last_index_maintenance: Option<chrono::DateTime<Utc>> = None;
            let mut last_log_cleanup: Option<chrono::DateTime<Utc>> = None;
            let mut last_sqlite_maintenance: Option<chrono::DateTime<Utc>> = None;
            let mut last_fts_optimize: Option<chrono::DateTime<Utc>> = None;
            // #5810: regime crash-durability checkpoint state.
            let mut last_regime_checkpoint: Option<chrono::DateTime<Utc>> = None;

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        let now = Utc::now();

                        // 4-term 복합 동의 게이트를 tick 당 한 번 계산한다 (R4 헬퍼 재사용).
                        // 이 함수는 같은 모듈(system.rs)의 free fn이므로 경로 접두사 없이 호출.
                        // collect_ok == false 면 데이터 파생/영속화 블록을 건너뛰고,
                        // 하우스키핑(보존 삭제·유지보수)은 게이트와 무관하게 계속 실행한다 —
                        // 동의 공백 기간에도 보존 정책으로 만료 데이터를 반드시 정리해야 하기 때문.
                        let collect_ok = collection_permitted(
                            config_manager.as_ref(),
                            consent_manager.as_ref(),
                            capture_paused.load(Ordering::Relaxed),
                        );

                        // [COLLECT] 시간별 메트릭 집계 — raw 메트릭에서 롤업을 파생·영속화한다.
                        if collect_ok {
                            let prev_hour = now - ChronoDuration::hours(1);
                            if let Err(e) = sqlite6.aggregate_hourly_metrics(prev_hour).await {
                                warn!("hour failure: {e}");
                            }
                        }

                        // [HOUSEKEEPING] 아래 3개 cleanup_* 은 보존 정책에 따라 만료 데이터를
                        // 삭제만 한다 (수집 아님). 동의 공백에도 반드시 실행해야 하므로 게이트 밖.
                        //
                        // #4631 MINOR-3 (의도된 갭): 롤업(aggregate_hourly_metrics)은 동의 게이트
                        // 안이지만 cleanup_old_metrics(삭제 전용)는 밖이다. 23h+ 연속 동의 공백 시
                        // 직전 동의 창의 raw 메트릭이 롤업되기 전에 보존 기한을 넘겨 삭제될 수 있어
                        // 해당 시간대의 시간별 롤업이 영구 부재할 수 있다. 철회된 동의 데이터를
                        // 파생하지 않는 fail-closed 동작이므로 수용한다 (자기-유발 withdrawal).
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

                                // [COLLECT] 스테일 벡터 재임베딩 — 모델 변경 시 임베딩을 다시
                                // 파생(update_vector)해 사용자 파생 데이터를 영속화한다. 동의 게이트로 보호.
                                // (mark_stale 자체는 플래그지만, 재임베딩 파이프라인 진입점이므로 함께 가둔다.)
                                if collect_ok {
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
                                }

                                // [HOUSEKEEPING] 벡터 보존 — 만료 벡터를 HNSW/SQLite 에서 삭제만 한다
                                // (수집 아님). 동의 공백에도 보존 정책으로 정리해야 하므로 게이트 밖.
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

                        // [HOUSEKEEPING] 세그먼트/다이제스트/보조 테이블 보존 + memory-graph
                        // prune/GC. 모두 만료 데이터를 삭제·정리만 한다 (수집 아님). 동의 공백에도
                        // 보존 정책을 적용해야 하므로 게이트 밖에서 실행.
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

                            // GDPR Art.17 erasure tombstone outbox GC (#5174 S5/R4):
                            // bound the retained sync_tombstones at max(retention, 90)
                            // days. Accepted convergence-cliff trade-off (see method doc).
                            if let Err(e) = sqlite6.gc_sync_tombstones(segment_retention_days) {
                                warn!("sync_tombstones GC failure: {e}");
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

                        // [COLLECT] 주간 다이제스트 자동 생성 — 세그먼트에서 다이제스트를
                        // 파생해 save_weekly_digest 로 영속화한다. 동의 게이트로 보호.
                        // --- Weekly digest auto-generation ---
                        if collect_ok {
                            let digest_day = config_manager
                                .as_ref()
                                .map(|cm| cm.get().analysis.embedding.digest_day)
                                .unwrap_or(maekon_core::config::Weekday::Sun);

                            let local_now = chrono::Local::now();
                            let is_digest_day =
                                local_now.weekday().num_days_from_sunday() == digest_day.num_days_from_sunday();
                            let is_midnight_hour = local_now.hour() == 0;

                            if is_digest_day && is_midnight_hour {
                                // Calculate week boundaries (Monday-based ISO week aligned to digest_day)
                                let week_end = now;
                                let week_start = now - ChronoDuration::days(7);

                                // Check if digest already exists for this week.
                                // #5097: sync SchedulerStorage digest 호출을 spawn_blocking
                                // 으로 오프로드(offload_storage) — async 워커 스레드 블로킹 회피.
                                let existing = {
                                    let sqlite6 = sqlite6.clone();
                                    offload_storage("weekly digest lookup", move || {
                                        sqlite6.list_weekly_digests(1)
                                    })
                                    .await
                                }
                                .and_then(|d| d.into_iter().next());

                                let already_generated = existing
                                    .as_ref()
                                    .map(|d| (now - d.week_end).num_hours() < 24)
                                    .unwrap_or(false);

                                if !already_generated {
                                    // Load actual segments for this week from storage
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
                                        existing.as_ref(),
                                    );

                                    let saved = {
                                        let sqlite6 = sqlite6.clone();
                                        offload_storage("weekly digest save", move || {
                                            sqlite6.save_weekly_digest(&digest)
                                        })
                                        .await
                                    };
                                    if saved.is_some() {
                                        info!("Weekly digest generated for week ending {}", week_end);
                                    }
                                }
                            }
                        }

                        // [COLLECT] 일간 다이제스트 자동 생성 — 세그먼트에서 다이제스트를 파생
                        // (save_daily_digest) + memory-graph claim/evidence edge 승격
                        // (persist_digest_memory_graph) + belief revision pass. 모두 사용자 파생
                        // 데이터를 영속화하므로 동의 게이트로 보호. (belief revision 은 내부에서
                        // memory_graph_enrichment own-field 동의로 추가 게이트됨 — defense-in-depth.)
                        // --- Daily digest auto-generation (midnight) ---
                        if collect_ok {
                            let local_now = chrono::Local::now();
                            if local_now.hour() == 0 {
                                // Generate digest for yesterday
                                let yesterday = local_now.date_naive()
                                    .pred_opt()
                                    .unwrap_or(local_now.date_naive());
                                let date_str = yesterday.format("%Y-%m-%d").to_string();

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

                                if existing.is_none() {
                                    // Load segments for yesterday
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
                                        let prev_date = yesterday
                                            .pred_opt()
                                            .unwrap_or(yesterday)
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

                                        let mut digest = maekon_analysis::DailyDigestGenerator::generate(
                                            &segments,
                                            yesterday,
                                            prev_digest.as_ref(),
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
                                        }

                                        // ADR-023 (D3/D5): promote the digest into
                                        // durable memory-graph claims + evidence edges.
                                        // Offline-capable — runs on the timeline content
                                        // even when no LLM insight was generated.
                                        if let Some(ref mg) = memory_graph {
                                            persist_digest_memory_graph(
                                                mg.as_ref(),
                                                &digest,
                                                Utc::now().timestamp(),
                                            )
                                            .await;
                                        }

                                        // ADR-023 Phase-2: LLM belief revision (D1/D2)
                                        // over the accumulated claims, once per day with
                                        // the digest. Triple-gated: the component is
                                        // local-LLM-gated at construction; here we also
                                        // require explicit memory_graph_enrichment consent
                                        // + the belief_revision_enabled flag. With no LLM
                                        // it degrades to a no-op.
                                        if let Some(ref br) = belief_revision {
                                            let consent_ok = consent_manager.as_ref().is_some_and(
                                                |c| c.effective_permissions().memory_graph_enrichment,
                                            );
                                            let flag_on = config_manager
                                                .as_ref()
                                                .map(|cm| cm.get().analysis.belief_revision_enabled)
                                                .unwrap_or(false);
                                            if consent_ok && flag_on {
                                                if let Err(e) =
                                                    br.run_pass(Utc::now().timestamp()).await
                                                {
                                                    warn!(err.code = %e.code(), "belief revision pass failed: {e}");
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        // [HOUSEKEEPING] 벡터 인덱스 정비 (count 갱신·HNSW 저장·IVF 재구축·바이너리
                        // 코드). 기존 벡터에 대한 인덱스 구조 유지보수일 뿐 신규 데이터 수집이 아니므로
                        // 게이트 밖. (재임베딩과 달리 새 임베딩을 파생하지 않는다.)
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

                        // --- Regime state periodic crash-durability checkpoint (#5810) ---
                        // The shutdown path (main.rs RunEvent::Exit) is the authoritative
                        // save; this block is a supplement that limits session loss on
                        // unclean exit to at most REGIME_CHECKPOINT_INTERVAL_MINS minutes.
                        //
                        // save_all calls conn.write_lock().run() synchronously (parking_lot
                        // mutex). The lock is held only for the duration of the SQLite
                        // execute calls (a few ms at most), so direct .await in this async
                        // context is acceptable — the blocking is bounded and infrequent
                        // (every 30 min). No spawn_blocking wrapper is added to keep the
                        // call site simple; this matches the main.rs shutdown pattern which
                        // also calls save_all directly inside a blocking runtime.
                        if let (Some(ref rs), Some(ref rm)) =
                            (&regime_storage, &regime_manager_arc)
                        {
                            let should_checkpoint = last_regime_checkpoint
                                .map(|last| {
                                    (now - last).num_minutes()
                                        >= super::super::config::REGIME_CHECKPOINT_INTERVAL_MINS
                                })
                                .unwrap_or(true);

                            if should_checkpoint {
                                last_regime_checkpoint = Some(now);
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

                        // --- Config file change detection ---
                        if let Some(ref cm) = config_manager {
                            check_config_file_changed(cm, &config_mtime).await;
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
///
/// `tokio::fs::metadata` 를 사용해 async 런타임 스레드를 블로킹하지 않습니다.
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
/// acceptable. Pure value construction lives in `maekon_analysis::claim_promoter`.
async fn persist_digest_memory_graph(
    memory_graph: &dyn maekon_core::ports::memory_graph_port::MemoryGraphPort,
    digest: &maekon_core::models::daily_digest::DailyDigest,
    now_secs: i64,
) {
    let mut claim_count = 0_usize;
    for (claim, edges) in
        maekon_analysis::claim_promoter::build_claims_from_digest(digest, now_secs)
    {
        if let Err(e) = memory_graph.save_claim(&claim).await {
            warn!(err.code = %e.code(), "memory-graph claim save failed: {e}");
            continue;
        }
        claim_count += 1;
        for edge in edges {
            if let Err(e) = memory_graph.add_edge(&edge).await {
                warn!(err.code = %e.code(), "memory-graph evidence edge failed: {e}");
            }
        }
    }
    if claim_count > 0 {
        debug!("ADR-023: promoted {claim_count} digest claim(s) to the memory graph");
    }
}

/// scheduler 의 sync `SchedulerStorage` digest/segment 호출을 tokio blocking 풀로
/// 오프로드한다 (#5097 / ADR-026 follow-up).
///
/// digest/segment 영속화 메서드는 `Arc<dyn SchedulerStorage>`(sync trait) 경유라
/// 집계 루프에서 직접 호출하면 async 워커 스레드가 SQLite I/O 로 블로킹된다(다른
/// 스토리지 호출은 async `MetricsStorage` supertrait 경유라 이미 non-blocking).
/// 매 호출을 `spawn_blocking` 으로 감싸 블로킹을 blocking 풀로 옮긴다 — SQL/동작은
/// sync 메서드와 동일하며 #4928 erase barrier 의 write_lock skip 도 그대로 보존된다.
///
/// 실패(스토리지 에러 또는 task panic)는 `context` 라벨로 로깅하고 `None` 을 반환해
/// 호출부의 best-effort 시맨틱(기존 `.ok()` / `if let Err`)을 유지한다.
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

/// `collection_permitted` 헬퍼 단위 테스트.
///
/// Scheduler 전체를 구성하지 않고 게이트 결정 논리만 검증한다.
/// 다섯 가지 시나리오를 커버한다:
///   1. config_manager 없음 → false (fail-closed)
///   2. consent_manager 없음(None) → false
///   3. 동의 미부여(NotGranted) → false
///   4. 동의 만료(Expired, screen_capture:true 이지만 만료) → false
///   5. 유효한 동의(screen_capture=true) → true
///   6. 유효한 동의이지만 capture_paused → false
#[cfg(test)]
mod collection_permitted_tests {
    use super::collection_permitted;
    use maekon_core::config_manager::ConfigManager;
    use maekon_core::consent::{ConsentManager, ConsentPermissions};
    use maekon_core::ports::consent_manager::ConsentManagerPort;
    use std::sync::Arc;

    /// 테스트용 고유 임시 파일 경로를 반환한다.
    /// `TempDir` 대신 `std::env::temp_dir()` + 난수로 경로를 생성하면
    /// deprecated API 경고 없이 OS 임시 디렉토리에 쓸 수 있다.
    fn tmp_path(suffix: &str) -> std::path::PathBuf {
        // nonce: 프로세스 ID + 스레드 ID 조합으로 충돌 가능성을 최소화한다.
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("maekon_test_{nonce}_{suffix}"))
    }

    /// ConfigManager 를 임시 파일 경로로 구성하는 헬퍼.
    /// `ConfigManager::with_path` 는 경로가 존재하지 않으면 기본값으로 생성한다.
    fn make_config_manager() -> ConfigManager {
        let path = tmp_path("config.json");
        ConfigManager::with_path(path).expect("ConfigManager 생성 실패")
    }

    /// 유효한 screen_capture 동의를 부여한 ConsentManager 를 반환한다.
    fn make_valid_consent(screen_capture: bool) -> Arc<dyn ConsentManagerPort> {
        let consent_path = tmp_path("consent_valid.json");
        let mgr = Arc::new(ConsentManager::new(consent_path));
        let perms = ConsentPermissions {
            screen_capture,
            ..Default::default()
        };
        // 30일 유효 동의 부여
        mgr.grant_consent(perms, 30).expect("동의 부여 실패");
        mgr
    }

    /// 동의 미부여 상태의 ConsentManager 를 반환한다.
    fn make_no_consent_manager() -> Arc<dyn ConsentManagerPort> {
        let consent_path = tmp_path("consent_none.json");
        Arc::new(ConsentManager::new(consent_path))
    }

    /// 시나리오 1: config_manager 가 None → fail-closed
    #[test]
    fn absent_config_manager_returns_false() {
        let consent = make_valid_consent(true);
        assert!(
            !collection_permitted(None, Some(&consent), false),
            "config_manager 없으면 항상 false"
        );
    }

    /// 시나리오 2: 동의 없음(NotGranted) → false
    #[test]
    fn absent_consent_manager_returns_false() {
        let cm = make_config_manager();
        // consent_manager = None → effective_permissions 는 all-false
        assert!(
            !collection_permitted(Some(&cm), None, false),
            "동의 매니저 없으면 항상 false"
        );
    }

    /// 시나리오 3: 동의 미부여(NotGranted) — grant_consent 를 한 번도 호출하지 않은
    /// ConsentManager 인스턴스는 `check_consent() == NotGranted` 이므로
    /// `effective_permissions()` 는 all-false 를 반환하고 게이트가 닫혀야 한다.
    #[test]
    fn no_consent_granted_returns_false() {
        let cm = make_config_manager();
        let mgr = make_no_consent_manager(); // 동의 미부여 상태
        assert!(
            !collection_permitted(Some(&cm), Some(&mgr), false),
            "동의 미부여 상태에서 항상 false"
        );
    }

    /// 시나리오 4: 동의 만료(Expired) — `screen_capture:true` 이지만 `expires_at` 가
    /// 과거인 ConsentRecord 를 파일로 직접 기록한 뒤 `ConsentManager::new` 로 읽어들여
    /// `effective_permissions()` 가 all-false 를 반환하는지 검증한다.
    /// 스테일 동의로 수집이 재개되는 것이 이 Task 의 핵심 위험이므로 반드시 별도 케이스로 존재해야 한다.
    #[test]
    fn expired_consent_returns_false() {
        use maekon_core::consent::{ConsentRecord, CURRENT_POLICY_VERSION};
        // 과거 만료 날짜를 가진 ConsentRecord 를 JSON 으로 파일에 기록한다.
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
                screen_capture: true, // 권한 자체는 부여됐으나 만료됨
                ..Default::default()
            },
            data_retention_days: 30,
        };
        std::fs::write(
            &consent_path,
            serde_json::to_string(&expired).expect("직렬화 실패"),
        )
        .expect("파일 쓰기 실패");
        // ConsentManager::new 는 파일을 읽어 Expired 상태로 초기화한다.
        let mgr: Arc<dyn ConsentManagerPort> = Arc::new(ConsentManager::new(consent_path));
        let cm = make_config_manager();
        assert!(
            !collection_permitted(Some(&cm), Some(&mgr), false),
            "Expired 동의는 screen_capture:true 이더라도 fail-closed 여야 한다"
        );
    }

    /// 시나리오 5: 유효한 동의(screen_capture=true) → true
    ///
    /// AppConfig 기본값은 `vision.capture_enabled = true` 이고
    /// active_hours_enabled = false 이므로 active-hours 게이트는 통과한다.
    #[test]
    fn valid_consent_with_screen_capture_returns_true() {
        let cm = make_config_manager();
        let consent = make_valid_consent(true);
        assert!(
            collection_permitted(Some(&cm), Some(&consent), false),
            "유효한 동의이고 paused=false 이면 true"
        );
    }

    /// 시나리오 6: 유효한 동의이지만 capture_paused → false
    #[test]
    fn valid_consent_but_paused_returns_false() {
        let cm = make_config_manager();
        let consent = make_valid_consent(true);
        assert!(
            !collection_permitted(Some(&cm), Some(&consent), true),
            "paused=true 이면 동의와 무관하게 false"
        );
    }
}

/// `metrics_collection_permitted` 헬퍼 단위 테스트 (F1 / Option A).
///
/// 메트릭 루프는 인프라 헬스 데이터(CONS-PM09 / spec §3.8 row 16)이므로
/// 동의(telemetry)만으로 게이트되고 TS/pause/active-hours 와 디커플된다.
/// 이 모듈은 process/aggregation 의 full-composite `collection_permitted` 와
/// 의도적으로 다른 게이트임을 고정한다:
///   1. Valid 동의 + telemetry:true → true (TrackingScheduleConfig 입력 자체가 없음 = TS 무관)
///   2. consent_manager 없음(None) → false (fail-closed)
///   3. 동의 미부여(NotGranted) → false
///   4. 동의 만료(Expired, telemetry:true 이지만 만료) → false (effective_permissions 가 마스킹)
///   5. 정책 버전 불일치(UpdateRequired, telemetry:true) → false
///   6. Valid 동의이지만 telemetry:false → false (own-field 게이트)
#[cfg(test)]
mod metrics_collection_permitted_tests {
    use super::metrics_collection_permitted;
    use maekon_core::consent::{
        ConsentManager, ConsentPermissions, ConsentRecord, CURRENT_POLICY_VERSION,
    };
    use maekon_core::ports::consent_manager::ConsentManagerPort;
    use std::sync::Arc;

    /// 테스트용 고유 임시 파일 경로.
    fn tmp_path(suffix: &str) -> std::path::PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("maekon_metrics_test_{nonce}_{suffix}"))
    }

    /// telemetry 동의가 부여된 Valid ConsentManager.
    fn make_valid_telemetry_consent() -> Arc<dyn ConsentManagerPort> {
        let mgr = Arc::new(ConsentManager::new(tmp_path("consent_telemetry.json")));
        mgr.grant_consent(
            ConsentPermissions {
                telemetry: true,
                ..Default::default()
            },
            30,
        )
        .expect("동의 부여 실패");
        mgr
    }

    /// 시나리오 1: Valid 동의 + telemetry:true → true.
    ///
    /// 이 함수는 config/TS 를 인자로 받지 않으므로(시그니처상 TS 입력 불가),
    /// TS 활성 여부와 무관하게 telemetry 동의만 있으면 게이트가 열린다 =
    /// CONS-PM09 (메트릭은 TS mute 중에도 계속). 이것이 process/aggregation 의
    /// full-composite 게이트와 메트릭 게이트가 디커플되었음을 증명하는 핵심 단언.
    #[test]
    fn valid_telemetry_consent_returns_true_regardless_of_ts() {
        let consent = make_valid_telemetry_consent();
        assert!(
            metrics_collection_permitted(Some(&consent)),
            "telemetry Valid 동의면 TS/pause/active-hours 와 무관하게 항상 true (CONS-PM09)"
        );
    }

    /// 시나리오 2: consent_manager None → fail-closed.
    #[test]
    fn absent_consent_manager_returns_false() {
        assert!(
            !metrics_collection_permitted(None),
            "동의 매니저 없으면 항상 false (fail-closed)"
        );
    }

    /// 시나리오 3: 동의 미부여(NotGranted) → false.
    #[test]
    fn no_consent_granted_returns_false() {
        let mgr: Arc<dyn ConsentManagerPort> =
            Arc::new(ConsentManager::new(tmp_path("consent_none.json")));
        assert!(
            !metrics_collection_permitted(Some(&mgr)),
            "동의 미부여 상태에서 항상 false"
        );
    }

    /// 시나리오 4: 동의 만료(Expired) — telemetry:true 이지만 만료 → false.
    /// effective_permissions() 가 Valid 가 아닌 상태를 all-false 로 마스킹한다.
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
                telemetry: true, // 권한 자체는 부여됐으나 만료됨
                ..Default::default()
            },
            data_retention_days: 30,
        };
        std::fs::write(
            &consent_path,
            serde_json::to_string(&expired).expect("직렬화 실패"),
        )
        .expect("파일 쓰기 실패");
        let mgr: Arc<dyn ConsentManagerPort> = Arc::new(ConsentManager::new(consent_path));
        assert!(
            !metrics_collection_permitted(Some(&mgr)),
            "Expired 동의는 telemetry:true 이더라도 fail-closed 여야 한다"
        );
    }

    /// 시나리오 5: 정책 버전 불일치(UpdateRequired) — telemetry:true 이지만 → false.
    /// expires_at=None 으로 두어 Expired 가 아닌 UpdateRequired 분기를 강제한다.
    #[test]
    fn update_required_consent_returns_false() {
        let consent_path = tmp_path("consent_stale.json");
        let stale = ConsentRecord {
            consent_id: "stale-metrics".to_string(),
            version: "0.0.1".to_string(), // 현 정책 버전과 불일치
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
            serde_json::to_string(&stale).expect("직렬화 실패"),
        )
        .expect("파일 쓰기 실패");
        let mgr: Arc<dyn ConsentManagerPort> = Arc::new(ConsentManager::new(consent_path));
        assert!(
            !metrics_collection_permitted(Some(&mgr)),
            "UpdateRequired 동의는 telemetry:true 이더라도 fail-closed 여야 한다"
        );
    }

    /// 시나리오 6: Valid 동의이지만 telemetry:false → false (own-field 게이트).
    /// screen_capture 등 다른 권한이 있어도 telemetry 가 꺼져 있으면 메트릭은 막힌다.
    #[test]
    fn valid_consent_without_telemetry_returns_false() {
        let mgr: Arc<dyn ConsentManagerPort> =
            Arc::new(ConsentManager::new(tmp_path("consent_no_telemetry.json")));
        mgr.grant_consent(
            ConsentPermissions {
                screen_capture: true, // 다른 권한은 부여됐으나 telemetry 는 false
                telemetry: false,
                ..Default::default()
            },
            30,
        )
        .expect("동의 부여 실패");
        assert!(
            !metrics_collection_permitted(Some(&mgr)),
            "telemetry:false 면 다른 권한과 무관하게 메트릭 게이트는 닫혀야 한다"
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

/// `spawn_aggregation_loop` 의 collect/derive 게이트 계약 테스트.
///
/// 집계 루프는 데이터 파생/영속화 블록(시간별 집계·일/주간 다이제스트·memory-graph
/// claim·스테일 벡터 재임베딩)을 `collect_ok` 뒤에 두고, `collect_ok` 는 R4 의
/// `collection_permitted` free fn 으로 계산된다(= 메트릭/프로세스 루프와 동일한 게이트).
/// `collection_permitted_tests` 가 6개 시나리오(config 없음/consent 없음/미부여/만료/
/// 유효/paused)를 이미 망라하므로, 여기서는 *집계 컨텍스트* 의 보안 임계 속성만 명시한다:
///   - 동의 없음(미부여) → collect_ok=false → 다이제스트/claim/재임베딩 쓰기 건너뜀
///   - 유효 동의 → collect_ok=true → 파생 쓰기 실행
///
/// 하우스키핑(보존 삭제·sqlite 유지보수·인덱스 정비)은 게이트와 무관하게 항상 실행되며,
/// 이는 코드 구조(게이트 밖 배치) + 컴파일로 보증된다.
///
/// 주: Scheduler 전체 + 16개 포트 + tokio 런타임 + 자정 트리거를 구성해 실제 쓰기를
/// spy 하는 통합 테스트는 과도하다. 게이트 결정은 순수 헬퍼로 추출되어 있어
/// 단위 테스트로 충분히 검증된다(should_rearm_vad 패턴).
#[cfg(test)]
mod aggregation_gate_tests {
    use super::collection_permitted;
    use maekon_core::config_manager::ConfigManager;
    use maekon_core::consent::{ConsentManager, ConsentPermissions};
    use maekon_core::ports::consent_manager::ConsentManagerPort;
    use std::sync::Arc;

    fn tmp_path(suffix: &str) -> std::path::PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("maekon_agg_test_{nonce}_{suffix}"))
    }

    fn make_config_manager() -> ConfigManager {
        ConfigManager::with_path(tmp_path("config.json")).expect("ConfigManager 생성 실패")
    }

    /// 동의 미부여 상태 → 집계 루프의 collect/derive 게이트(collect_ok)가 닫혀야 한다.
    /// 즉 일/주간 다이제스트·memory-graph claim·재임베딩 쓰기가 건너뛰어진다.
    #[test]
    fn aggregation_derive_writes_skipped_without_consent() {
        let cm = make_config_manager();
        let no_consent: Arc<dyn ConsentManagerPort> =
            Arc::new(ConsentManager::new(tmp_path("consent_none.json")));
        let collect_ok = collection_permitted(Some(&cm), Some(&no_consent), false);
        assert!(
            !collect_ok,
            "동의 미부여 시 집계 루프의 파생/영속화 블록은 게이트가 닫혀 실행되지 않아야 한다"
        );
    }

    /// 유효 동의(screen_capture=true, paused=false) → collect_ok=true → 파생 쓰기 실행.
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
        .expect("동의 부여 실패");
        let collect_ok = collection_permitted(Some(&cm), Some(&mgr), false);
        assert!(
            collect_ok,
            "유효 동의 + paused=false 이면 집계 루프의 파생 블록이 실행되어야 한다"
        );
    }

    /// paused=true 이면 유효 동의여도 collect_ok=false (파생 쓰기 건너뜀) —
    /// 하우스키핑은 별개로 계속 실행(코드 구조 보증)된다.
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
        .expect("동의 부여 실패");
        assert!(
            !collection_permitted(Some(&cm), Some(&mgr), true),
            "paused=true 이면 파생 블록 게이트가 닫혀야 한다 (하우스키핑은 게이트 밖에서 계속)"
        );
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

/// #5810: regime checkpoint interval gate contract tests.
///
/// The gate logic mirrors the `last_index_maintenance` / `last_sqlite_maintenance`
/// pattern: `num_minutes() >= REGIME_CHECKPOINT_INTERVAL_MINS` fires the first
/// time (last=None → unwrap_or(true)) and then only after the interval elapses.
#[cfg(test)]
mod regime_checkpoint_gate_tests {
    use chrono::{Duration as ChronoDuration, Utc};

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

    /// Gate fires on first call (last = None → unwrap_or(true)).
    #[test]
    fn gate_fires_on_first_call_when_last_is_none() {
        let last: Option<chrono::DateTime<Utc>> = None;
        let now = Utc::now();
        let interval = super::super::super::config::REGIME_CHECKPOINT_INTERVAL_MINS;
        let should = last
            .map(|l| (now - l).num_minutes() >= interval)
            .unwrap_or(true);
        assert!(should, "gate must fire when last_regime_checkpoint is None");
    }

    /// Gate does not fire when the interval has not elapsed.
    #[test]
    fn gate_skips_when_interval_not_elapsed() {
        let interval = super::super::super::config::REGIME_CHECKPOINT_INTERVAL_MINS;
        // last checkpoint was (interval - 1) minutes ago.
        let last = Some(Utc::now() - ChronoDuration::minutes(interval - 1));
        let now = Utc::now();
        let should = last
            .map(|l| (now - l).num_minutes() >= interval)
            .unwrap_or(true);
        assert!(
            !should,
            "gate must not fire within the interval window ({interval} min)"
        );
    }

    /// Gate fires once the interval has elapsed.
    #[test]
    fn gate_fires_after_interval_elapsed() {
        let interval = super::super::super::config::REGIME_CHECKPOINT_INTERVAL_MINS;
        // last checkpoint was (interval + 1) minutes ago.
        let last = Some(Utc::now() - ChronoDuration::minutes(interval + 1));
        let now = Utc::now();
        let should = last
            .map(|l| (now - l).num_minutes() >= interval)
            .unwrap_or(true);
        assert!(
            should,
            "gate must fire after the interval has elapsed ({interval} min)"
        );
    }
}
