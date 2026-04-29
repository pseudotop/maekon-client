use chrono::Utc;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};

use super::super::config::PlatformEgressPolicy;
use super::super::Scheduler;

impl Scheduler {
    #[tracing::instrument(skip_all)]
    pub(in crate::scheduler) fn spawn_sync_loop(
        &self,
        sync_interval: Duration,
        egress_policy: Arc<PlatformEgressPolicy>,
        mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) -> tokio::task::JoinHandle<()> {
        let uploader4 = self.batch_sink.clone();
        let storage4 = self.storage.clone();
        let frame_storage4 = self.frame_storage.clone();
        let egress4 = egress_policy;

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(sync_interval);

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        if let Some(ref sink) = uploader4 {
                            if egress4.is_enabled() {
                                match sink.flush().await {
                                    Ok(count) => {
                                        if count > 0 {
                                            debug!("batch: {count}items sent");
                                            if let Err(e) = storage4.mark_unsent_as_sent_before(Utc::now()).await {
                                                warn!(err.code = %e.code(), "mark sent failure: {e}");
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        warn!(err.code = %e.code(), "batch failure: {e}");
                                    }
                                }
                            }
                        }

                        // Log events dropped during this flush cycle
                        if let Some(ref sink) = uploader4 {
                            let dropped = sink.take_dropped_since_last();
                            if dropped > 0 {
                                warn!(count = dropped, "events dropped during flush cycle");
                            }
                        }

                        if let Err(e) = storage4.enforce_retention().await {
                            warn!(err.code = %e.code(), "event policy failure: {e}");
                        }

                        if let Some(ref fs) = frame_storage4 {
                            if let Err(e) = fs.enforce_retention().await {
                                warn!(err.code = %e.code(), "frame policy failure: {e}");
                            }
                            if let Err(e) = fs.enforce_storage_limit().await {
                                warn!(err.code = %e.code(), "frame failure: {e}");
                            }
                        }
                    }
                    _ = shutdown_rx.changed() => {
                        // Flush remaining events before shutdown
                        if let Some(ref sink) = uploader4 {
                            if egress4.is_enabled() {
                                loop {
                                    match sink.flush().await {
                                        Ok(0) => break,
                                        Ok(count) => {
                                            info!("shutdown flush: {count} events sent");
                                        }
                                        Err(e) => {
                                            warn!(err.code = %e.code(), "shutdown flush failed: {e}");
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                        info!("ended");
                        break;
                    }
                }
            }
        })
    }

    #[tracing::instrument(skip_all)]
    pub(in crate::scheduler) fn spawn_heartbeat_loop(
        &self,
        heartbeat_interval: Duration,
        session_id: String,
        egress_policy: Arc<PlatformEgressPolicy>,
        mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) -> tokio::task::JoinHandle<()> {
        let api = self.api_client.clone();
        let sid = session_id;

        tokio::spawn(async move {
            let api = match api {
                Some(a) => a,
                None => {
                    let _ = shutdown_rx.changed().await;
                    return;
                }
            };

            if !egress_policy.is_enabled() {
                let _ = shutdown_rx.changed().await;
                return;
            }

            let mut interval = tokio::time::interval(heartbeat_interval);

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        if let Err(e) = api.send_heartbeat(&sid).await {
                            warn!(err.code = %e.code(), "heartbeat failure: {e}");
                        }
                    }
                    _ = shutdown_rx.changed() => {
                        info!("heartbeat ended");
                        break;
                    }
                }
            }
        })
    }
}
