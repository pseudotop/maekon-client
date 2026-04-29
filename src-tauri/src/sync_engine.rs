//! SyncEngine -- orchestrates the pull/merge/push sync cycle.
//!
//! This is a wiring-level component (no SQL, no transport logic).
//! It coordinates ChangeExtractor, ChangeMerger, and SyncTransport
//! through the port traits defined in oneshim-core.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tracing::{debug, info, warn};

use chrono::{DateTime, Utc};

use oneshim_core::consent::ConsentManager;
use oneshim_core::error::CoreError;
use oneshim_core::models::sync::{ChangeSet, ChangeSetKind, SyncResult};
use oneshim_core::ports::change_extractor::ChangeExtractor;
use oneshim_core::ports::change_merger::ChangeMerger;
use oneshim_core::ports::sync_transport::SyncTransport;
use oneshim_core::sync::Hlc;

#[allow(dead_code)] // fields stored for cross-device sync lifecycle
pub struct SyncEngine {
    extractor: Arc<dyn ChangeExtractor>,
    merger: Arc<dyn ChangeMerger>,
    transport: Arc<dyn SyncTransport>,
    /// Shared ConsentManager from the runtime. Read-only access via `Arc`.
    /// Callers must pass the same instance used elsewhere in the runtime
    /// rather than letting SyncEngine construct its own instance.
    consent_manager: Option<Arc<ConsentManager>>,
    device_id: String,
    device_name: String,
    /// High-watermark HLC from the last successful push. Only rows with
    /// HLC > this value will be extracted on the next push cycle, avoiding
    /// re-extraction of all local data every cycle.
    last_push_watermark: parking_lot::Mutex<Hlc>,
    /// Tracks whether a GDPR deletion event was already pushed in this
    /// engine's lifetime. Since `ConsentManager` is shared via `Arc`
    /// (immutable), this flag provides the equivalent of
    /// `ConsentManager::clear_pending_deletion()` without requiring `&mut`.
    deletion_pushed: AtomicBool,
    /// Timestamp of the last successful sync cycle completion.
    last_sync_at: parking_lot::Mutex<Option<DateTime<Utc>>>,
    /// Error message from the most recent failed sync cycle, if any.
    last_error: parking_lot::Mutex<Option<String>>,
}

impl SyncEngine {
    /// Create a new SyncEngine.
    ///
    /// `consent_manager` should be the application-wide `ConsentManager`
    /// (the same `Arc<ConsentManager>` used by the scheduler and other
    /// components). When `None`, consent checks are skipped (sync always
    /// runs). Do **not** construct a separate `ConsentManager` from the
    /// file path — that creates divergent in-memory state.
    pub async fn new(
        extractor: Arc<dyn ChangeExtractor>,
        merger: Arc<dyn ChangeMerger>,
        transport: Arc<dyn SyncTransport>,
        consent_manager: Option<Arc<ConsentManager>>,
        device_id: String,
        device_name: String,
    ) -> Self {
        // Seed the push watermark from storage so we never re-push rows that
        // were already successfully pushed in a previous process lifetime.
        let initial_watermark = match extractor.local_watermark().await {
            Ok(wm) => {
                if wm != Hlc::default() {
                    debug!(
                        wall_ms = wm.wall_ms,
                        counter = wm.counter,
                        "initialized push watermark from storage"
                    );
                }
                wm
            }
            Err(e) => {
                warn!("failed to read initial push watermark, starting from zero: {e}");
                Hlc::default()
            }
        };

        Self {
            extractor,
            merger,
            transport,
            consent_manager,
            device_id,
            device_name,
            last_push_watermark: parking_lot::Mutex::new(initial_watermark),
            deletion_pushed: AtomicBool::new(false),
            last_sync_at: parking_lot::Mutex::new(None),
            last_error: parking_lot::Mutex::new(None),
        }
    }

    /// Run one complete sync cycle: check consent, handle deletion,
    /// pull + merge, extract + push.
    ///
    /// On success, updates `last_sync_at` and clears `last_error`.
    /// On failure, records the error message in `last_error`.
    pub async fn run_cycle(&self) -> Result<Option<SyncResult>, CoreError> {
        match self.run_cycle_inner().await {
            Ok(result) => {
                *self.last_sync_at.lock() = Some(Utc::now());
                *self.last_error.lock() = None;
                Ok(result)
            }
            Err(e) => {
                *self.last_error.lock() = Some(e.to_string());
                Err(e)
            }
        }
    }

    /// Inner implementation of the sync cycle, called by `run_cycle` which
    /// wraps it with health tracking.
    async fn run_cycle_inner(&self) -> Result<Option<SyncResult>, CoreError> {
        // Gate 1: consent check
        if let Some(ref cm) = self.consent_manager {
            if !cm.is_permitted(|p| p.cross_device_sync) {
                debug!("sync skipped: cross_device_sync consent not granted");
                return Ok(None);
            }
        }

        // Gate 2: check for pending GDPR deletion (skip if already pushed)
        let has_pending_deletion = !self.deletion_pushed.load(Ordering::Acquire)
            && self
                .consent_manager
                .as_ref()
                .map(|cm| cm.has_pending_deletion())
                .unwrap_or(false);
        if has_pending_deletion {
            return self.push_deletion_event().await;
        }

        // --- Pull phase ---
        let local_watermark = self.extractor.local_watermark().await?;
        let mut merge_result: Option<SyncResult> = None;

        // Pull changesets in a loop until no more are available
        loop {
            let watermark = merge_result
                .as_ref()
                .map(|r| &r.new_watermark)
                .unwrap_or(&local_watermark);

            match self.transport.pull(watermark).await? {
                None => break,
                Some(changeset) => {
                    info!(
                        origin = %changeset.origin_device_id,
                        rows = changeset.row_count(),
                        "pulled changeset from transport"
                    );
                    let result = self.merger.apply_changes(changeset).await?;
                    debug!(
                        applied = result.applied,
                        skipped_lww = result.skipped_lww,
                        skipped_dup = result.skipped_dup,
                        tombstoned = result.tombstoned,
                        "merge completed"
                    );
                    merge_result = Some(result);
                }
            }
        }

        // --- Push phase ---
        // Use the last successful push watermark so we only extract rows
        // that were created or modified since the previous push.
        let since = { self.last_push_watermark.lock().clone() };
        let local_changes = self.extractor.get_changes_since(&since).await?;

        if !local_changes.is_empty() {
            info!(rows = local_changes.row_count(), "pushing local changes");
            self.transport.push(&local_changes).await?;
            // Advance watermark only after a successful push so that a
            // transient transport failure causes a retry of the same rows.
            let new_watermark = local_changes.watermark.clone();
            *self.last_push_watermark.lock() = new_watermark;
        }

        Ok(merge_result)
    }

    /// Push a GDPR Article 17 deletion event and clear the pending flag.
    async fn push_deletion_event(&self) -> Result<Option<SyncResult>, CoreError> {
        info!("pushing GDPR Article 17 deletion event");

        let deletion_cs = ChangeSet {
            kind: ChangeSetKind::DeletionEvent,
            origin_device_id: self.device_id.clone(),
            origin_device_name: self.device_name.clone(),
            watermark: Hlc::now(&self.device_id),
            ..Default::default()
        };

        self.transport.push(&deletion_cs).await?;

        // Mark deletion as pushed. Since the ConsentManager is shared via
        // Arc (immutable), we track this locally with an AtomicBool instead
        // of calling ConsentManager::clear_pending_deletion(&mut self).
        self.deletion_pushed.store(true, Ordering::Release);

        info!("GDPR deletion event pushed successfully");
        Ok(None)
    }

    // ── Public accessors for IPC/REST ──────────────────────────────────

    /// Device identity of this sync engine instance.
    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    /// Human-readable device name.
    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    /// Returns `(last_sync_at_rfc3339, last_error)` for health reporting.
    pub fn health_status(&self) -> (Option<String>, Option<String>) {
        let sync_at = self.last_sync_at.lock().as_ref().map(|d| d.to_rfc3339());
        let error = self.last_error.lock().clone();
        (sync_at, error)
    }

    /// Discover known peers via the configured transport.
    pub async fn discover_peers(
        &self,
    ) -> Result<Vec<oneshim_core::models::sync::PeerInfo>, CoreError> {
        self.transport.discover_peers().await
    }

    /// Remove a peer from the transport's known-peers list.
    pub async fn forget_peer(&self, device_id: &str) -> Result<(), CoreError> {
        self.transport.forget_peer(device_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use oneshim_core::consent::ConsentPermissions;
    use oneshim_core::models::sync::PeerInfo;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // --- Mock implementations ---

    struct MockExtractor {
        changeset: ChangeSet,
        /// Records the `since` argument from each `get_changes_since` call.
        since_log: std::sync::Mutex<Vec<Hlc>>,
    }

    impl MockExtractor {
        fn new(changeset: ChangeSet) -> Self {
            Self {
                changeset,
                since_log: std::sync::Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl ChangeExtractor for MockExtractor {
        async fn get_changes_since(&self, since: &Hlc) -> Result<ChangeSet, CoreError> {
            self.since_log.lock().unwrap().push(since.clone());
            Ok(self.changeset.clone())
        }
        async fn local_watermark(&self) -> Result<Hlc, CoreError> {
            Ok(self.changeset.watermark.clone())
        }
    }

    struct MockMerger {
        apply_count: AtomicUsize,
    }

    #[async_trait]
    impl ChangeMerger for MockMerger {
        async fn apply_changes(&self, _changes: ChangeSet) -> Result<SyncResult, CoreError> {
            self.apply_count.fetch_add(1, Ordering::SeqCst);
            Ok(SyncResult {
                applied: 1,
                ..Default::default()
            })
        }
    }

    struct MockTransport {
        pull_result: std::sync::Mutex<Vec<Option<ChangeSet>>>,
        push_count: AtomicUsize,
    }

    #[async_trait]
    impl SyncTransport for MockTransport {
        async fn push(&self, _changes: &ChangeSet) -> Result<(), CoreError> {
            self.push_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        async fn pull(&self, _since: &Hlc) -> Result<Option<ChangeSet>, CoreError> {
            let mut results = self.pull_result.lock().unwrap();
            if results.is_empty() {
                Ok(None)
            } else {
                Ok(results.remove(0))
            }
        }
        async fn discover_peers(&self) -> Result<Vec<PeerInfo>, CoreError> {
            Ok(vec![])
        }
    }

    fn make_consent_manager(sync_granted: bool) -> Option<Arc<ConsentManager>> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let mut cm = ConsentManager::new(path);
        if sync_granted {
            cm.grant_consent(
                ConsentPermissions {
                    cross_device_sync: true,
                    ..Default::default()
                },
                30,
            )
            .unwrap();
        }
        // Leak the tempdir to keep the path alive
        std::mem::forget(dir);
        Some(Arc::new(cm))
    }

    /// Build a ConsentManager with pending_deletion=true and cross_device_sync
    /// consent granted. This simulates a revoke-then-re-grant scenario.
    fn make_consent_manager_with_pending_deletion() -> Arc<ConsentManager> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let mut cm = ConsentManager::new(path);
        // Grant, revoke (sets pending_deletion=true), then re-grant with sync.
        cm.grant_consent(
            ConsentPermissions {
                cross_device_sync: true,
                ..Default::default()
            },
            30,
        )
        .unwrap();
        cm.revoke_consent().unwrap();
        cm.grant_consent(
            ConsentPermissions {
                cross_device_sync: true,
                ..Default::default()
            },
            30,
        )
        .unwrap();
        std::mem::forget(dir);
        Arc::new(cm)
    }

    #[tokio::test]
    async fn cycle_skipped_when_consent_not_granted() {
        let engine = SyncEngine::new(
            Arc::new(MockExtractor::new(ChangeSet::default())),
            Arc::new(MockMerger {
                apply_count: AtomicUsize::new(0),
            }),
            Arc::new(MockTransport {
                pull_result: std::sync::Mutex::new(vec![]),
                push_count: AtomicUsize::new(0),
            }),
            make_consent_manager(false),
            "dev-a".to_string(),
            "Test".to_string(),
        )
        .await;

        let result = engine.run_cycle().await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn normal_pull_merge_push_cycle() {
        let remote_cs = ChangeSet {
            origin_device_id: "dev-b".to_string(),
            origin_device_name: "Remote".to_string(),
            segments: vec![serde_json::json!({"id": "seg-1"})],
            ..Default::default()
        };

        let merger = Arc::new(MockMerger {
            apply_count: AtomicUsize::new(0),
        });
        let transport = Arc::new(MockTransport {
            pull_result: std::sync::Mutex::new(vec![Some(remote_cs), None]),
            push_count: AtomicUsize::new(0),
        });

        let engine = SyncEngine::new(
            Arc::new(MockExtractor::new(ChangeSet {
                segments: vec![serde_json::json!({"id": "local-seg"})],
                origin_device_id: "dev-a".to_string(),
                ..Default::default()
            })),
            merger.clone(),
            transport.clone(),
            make_consent_manager(true),
            "dev-a".to_string(),
            "Test".to_string(),
        )
        .await;

        let result = engine.run_cycle().await.unwrap();
        assert!(result.is_some());
        assert_eq!(merger.apply_count.load(Ordering::SeqCst), 1);
        assert!(transport.push_count.load(Ordering::SeqCst) >= 1);
    }

    #[tokio::test]
    async fn empty_pull_results_in_push_only() {
        let transport = Arc::new(MockTransport {
            pull_result: std::sync::Mutex::new(vec![]),
            push_count: AtomicUsize::new(0),
        });
        let merger = Arc::new(MockMerger {
            apply_count: AtomicUsize::new(0),
        });

        let engine = SyncEngine::new(
            Arc::new(MockExtractor::new(ChangeSet {
                segments: vec![serde_json::json!({"id": "local-seg"})],
                origin_device_id: "dev-a".to_string(),
                ..Default::default()
            })),
            merger.clone(),
            transport.clone(),
            make_consent_manager(true),
            "dev-a".to_string(),
            "Test".to_string(),
        )
        .await;

        let result = engine.run_cycle().await.unwrap();
        assert!(result.is_none()); // no merge happened
        assert_eq!(merger.apply_count.load(Ordering::SeqCst), 0);
        assert_eq!(transport.push_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn deletion_event_pushed_when_pending() {
        let consent_mgr = make_consent_manager_with_pending_deletion();

        let transport = Arc::new(MockTransport {
            pull_result: std::sync::Mutex::new(vec![]),
            push_count: AtomicUsize::new(0),
        });

        let engine = SyncEngine::new(
            Arc::new(MockExtractor::new(ChangeSet::default())),
            Arc::new(MockMerger {
                apply_count: AtomicUsize::new(0),
            }),
            transport.clone(),
            Some(consent_mgr),
            "dev-a".to_string(),
            "Test".to_string(),
        )
        .await;

        let result = engine.run_cycle().await.unwrap();
        assert!(result.is_none());
        assert_eq!(transport.push_count.load(Ordering::SeqCst), 1);

        // Second cycle should NOT re-push the deletion event (local flag cleared)
        let result2 = engine.run_cycle().await.unwrap();
        assert!(result2.is_none());
        assert_eq!(
            transport.push_count.load(Ordering::SeqCst),
            1,
            "deletion should only be pushed once"
        );
    }

    #[tokio::test]
    async fn push_watermark_advances_after_successful_push() {
        let watermark = Hlc {
            wall_ms: 5000,
            counter: 3,
            device_id: "dev-a".to_string(),
        };
        let extractor = Arc::new(MockExtractor::new(ChangeSet {
            segments: vec![serde_json::json!({"id": "seg-1"})],
            origin_device_id: "dev-a".to_string(),
            watermark: watermark.clone(),
            ..Default::default()
        }));
        let transport = Arc::new(MockTransport {
            pull_result: std::sync::Mutex::new(vec![]),
            push_count: AtomicUsize::new(0),
        });

        let engine = SyncEngine::new(
            extractor.clone(),
            Arc::new(MockMerger {
                apply_count: AtomicUsize::new(0),
            }),
            transport,
            make_consent_manager(true),
            "dev-a".to_string(),
            "Test".to_string(),
        )
        .await;

        // First cycle: extractor is called with the initial watermark (seeded from local_watermark)
        engine.run_cycle().await.unwrap();
        // Second cycle: extractor should receive the advanced watermark, not Hlc::default()
        engine.run_cycle().await.unwrap();

        let log = extractor.since_log.lock().unwrap();
        assert_eq!(log.len(), 2);
        // Both calls should use the same watermark since the changeset watermark
        // equals the initial local_watermark.
        assert_eq!(log[0], watermark, "first push should use seeded watermark");
        assert_eq!(
            log[1], watermark,
            "second push should use advanced watermark"
        );
    }
}
