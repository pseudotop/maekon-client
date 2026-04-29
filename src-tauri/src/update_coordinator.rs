use crate::updater::{UpdateCheckResult, UpdateError, Updater};
use async_trait::async_trait;
use chrono::Utc;
use oneshim_api_contracts::update::DownloadProgress;
use oneshim_core::config::UpdateConfig;
use oneshim_web::update_control::{PendingUpdateInfo, UpdateAction, UpdatePhase, UpdateStatus};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, watch, RwLock};
use tracing::{debug, error, info, warn};

#[async_trait]
pub trait UpdateExecutor: Send + Sync {
    fn should_check_for_updates(&self) -> bool;
    async fn check_for_updates(&self) -> Result<UpdateCheckResult, UpdateError>;
    fn save_last_check_time(&self) -> Result<(), UpdateError>;
    async fn download_update(&self, download_url: &str) -> Result<PathBuf, UpdateError>;

    /// Stream download with progress reporting via a watch channel.
    /// Default implementation delegates to `download_update` (no progress).
    async fn download_update_with_progress(
        &self,
        download_url: &str,
        progress_tx: watch::Sender<DownloadProgress>,
    ) -> Result<PathBuf, UpdateError> {
        let result = self.download_update(download_url).await?;
        // Signal completion
        let _ = progress_tx.send(DownloadProgress {
            bytes_downloaded: 0,
            total_bytes: 0,
            percent: 100.0,
        });
        Ok(result)
    }

    fn install_and_restart(&self, downloaded_path: &Path) -> Result<(), UpdateError>;
}

#[async_trait]
impl UpdateExecutor for Updater {
    fn should_check_for_updates(&self) -> bool {
        self.should_check_for_updates()
    }

    async fn check_for_updates(&self) -> Result<UpdateCheckResult, UpdateError> {
        self.check_for_updates().await
    }

    fn save_last_check_time(&self) -> Result<(), UpdateError> {
        self.save_last_check_time()
    }

    async fn download_update(&self, download_url: &str) -> Result<PathBuf, UpdateError> {
        self.download_update(download_url).await
    }

    async fn download_update_with_progress(
        &self,
        download_url: &str,
        progress_tx: watch::Sender<DownloadProgress>,
    ) -> Result<PathBuf, UpdateError> {
        self.download_update_with_progress(download_url, progress_tx)
            .await
    }

    fn install_and_restart(&self, downloaded_path: &Path) -> Result<(), UpdateError> {
        self.install_and_restart(downloaded_path)
    }
}

pub fn initial_status(config: &UpdateConfig, auto_install: bool) -> UpdateStatus {
    UpdateStatus {
        enabled: config.enabled,
        auto_install,
        phase: UpdatePhase::Idle,
        message: None,
        pending: None,
        download_progress: None,
        rollback: None,
        revision: 0,
        updated_at: Utc::now().to_rfc3339(),
    }
}

pub async fn run_update_coordinator(
    config: UpdateConfig,
    state: Arc<RwLock<UpdateStatus>>,
    action_rx: mpsc::UnboundedReceiver<UpdateAction>,
    status_tx: Option<broadcast::Sender<UpdateStatus>>,
    auto_install: bool,
) {
    if !config.enabled {
        let mut guard = state.write().await;
        guard.phase = UpdatePhase::Idle;
        guard.message = Some("Update feature is disabled".to_string());
        guard.pending = None;
        guard.touch();
        if let Some(tx) = &status_tx {
            if let Err(e) = tx.send(guard.clone()) {
                debug!("channel send failed: {e}");
            }
        }
        return;
    }

    let check_interval_hours = config.check_interval_hours;

    // D10 spawn-order guard (Phase 4): the update-check coordinator must not
    // start until app_runtime_launch.rs:66-74 has persisted installation_id.
    // This is guaranteed by current launch order (config + UUID are
    // synchronously committed before any update task spawns), but a future
    // regression would silently hide the device from rollout via D10's
    // defensive None handling in updater/mod.rs:check_for_updates_from.
    // Surface the invariant loudly in both debug and release builds.
    if config.installation_id.is_none() {
        // Loop 3 iter 1 fix (I-5): removed `debug_assert!(false, ...)` —
        // panicking dev builds on an invariant violation was a foot-gun for
        // test fixtures / Tauri dev runs and did not actually protect users
        // (release builds only get the tracing event).
        //
        // The single source of regression detection is the tracing::error!
        // below — captured as an OTel span event when the `telemetry`
        // feature is active. If a future counter API lands, add the
        // increment here (same namespace used symmetrically in Task 9's
        // rollback handler).
        tracing::error!(
            "update-check coordinator started with installation_id = None; \
             rollout gate will exclude this device until next launch"
        );
    }

    let updater = Updater::new(config);

    run_update_coordinator_with_executor(
        updater,
        state,
        action_rx,
        status_tx,
        auto_install,
        check_interval_hours,
    )
    .await;
}

pub async fn run_update_coordinator_with_executor<E: UpdateExecutor + 'static>(
    updater: E,
    state: Arc<RwLock<UpdateStatus>>,
    mut action_rx: mpsc::UnboundedReceiver<UpdateAction>,
    status_tx: Option<broadcast::Sender<UpdateStatus>>,
    auto_install: bool,
    check_interval_hours: u32,
) {
    // Track the downloaded file path between the two phases
    let mut downloaded_path: Option<PathBuf> = None;

    if let Some(tx) = &status_tx {
        let snapshot = state.read().await.clone();
        if let Err(e) = tx.send(snapshot) {
            debug!("channel send failed: {e}");
        }
    }

    // Initial check on startup
    if updater.should_check_for_updates() {
        run_check(
            &updater,
            &state,
            status_tx.as_ref(),
            auto_install,
            &mut downloaded_path,
        )
        .await;
    }

    // Periodic background re-check: use config's check_interval_hours,
    // clamped to minimum 1 hour to avoid API rate limits.
    let recheck_secs = (check_interval_hours.max(1) as u64) * 3600;
    let recheck_interval = std::time::Duration::from_secs(recheck_secs);
    let mut recheck_timer = tokio::time::interval(recheck_interval);
    recheck_timer.tick().await; // consume the immediate first tick

    loop {
        tokio::select! {
            action = action_rx.recv() => {
                let Some(action) = action else { break };
                match action {
                    UpdateAction::CheckNow => {
                        run_check(&updater, &state, status_tx.as_ref(), auto_install, &mut downloaded_path).await;
                    }
                    UpdateAction::Approve => {
                        let current_phase = state.read().await.phase.clone();
                        match current_phase {
                            UpdatePhase::PendingApproval => {
                                // Phase 1: start download
                                if let Err(e) = run_download(&updater, &state, status_tx.as_ref(), &mut downloaded_path).await {
                                    emit_error(&state, status_tx.as_ref(), &format!("Download failed: {e}")).await;
                                } else if auto_install {
                                    // Auto-install: proceed to installation immediately
                                    if let Err(e) = run_install(&updater, &state, status_tx.as_ref(), &mut downloaded_path).await {
                                        emit_error(&state, status_tx.as_ref(), &format!("Auto-install failed: {e}")).await;
                                    }
                                }
                            }
                            UpdatePhase::ReadyToInstall => {
                                // Phase 2: install from downloaded file
                                if let Err(e) = run_install(&updater, &state, status_tx.as_ref(), &mut downloaded_path).await {
                                    emit_error(&state, status_tx.as_ref(), &format!("Installation failed: {e}")).await;
                                }
                            }
                            _ => {
                                debug!("Approve action ignored in phase {:?}", current_phase);
                            }
                        }
                    }
                    UpdateAction::Defer => {
                        let current_phase = state.read().await.phase.clone();
                        match current_phase {
                            UpdatePhase::PendingApproval | UpdatePhase::ReadyToInstall => {
                                downloaded_path = None;
                                let mut guard = state.write().await;
                                guard.phase = UpdatePhase::Deferred;
                                guard.message = Some("Update was deferred".to_string());
                                guard.pending = None;
                                guard.download_progress = None;
                                guard.touch();
                                if let Some(tx) = &status_tx {
                                    if let Err(e) = tx.send(guard.clone()) {
                                        debug!("channel send failed: {e}");
                                    }
                                }
                            }
                            _ => {
                                debug!("Defer action ignored in phase {:?}", current_phase);
                            }
                        }
                    }
                }
            }
            _ = recheck_timer.tick() => {
                // Skip re-check if an update is already pending, downloading, or installing
                let phase = state.read().await.phase.clone();
                if matches!(
                    phase,
                    UpdatePhase::Idle | UpdatePhase::Deferred | UpdatePhase::Error
                ) {
                    info!("periodic update re-check");
                    run_check(&updater, &state, status_tx.as_ref(), auto_install, &mut downloaded_path).await;
                }
            }
        }
    }
}

/// Phase 1: Download the update with streaming progress.
async fn run_download<E: UpdateExecutor>(
    updater: &E,
    state: &Arc<RwLock<UpdateStatus>>,
    status_tx: Option<&broadcast::Sender<UpdateStatus>>,
    downloaded_path: &mut Option<PathBuf>,
) -> Result<(), UpdateError> {
    let pending = {
        let guard = state.read().await;
        guard.pending.clone()
    };

    let Some(pending) = pending else {
        let mut guard = state.write().await;
        guard.phase = UpdatePhase::Idle;
        guard.message = Some("No pending update to download".to_string());
        guard.download_progress = None;
        guard.touch();
        broadcast_status(status_tx, &guard);
        return Ok(());
    };

    // Transition to Downloading
    {
        let mut guard = state.write().await;
        guard.phase = UpdatePhase::Downloading;
        guard.message = Some(format!("Downloading version {}", pending.latest_version));
        guard.download_progress = Some(DownloadProgress {
            bytes_downloaded: 0,
            total_bytes: 0,
            percent: 0.0,
        });
        guard.touch();
        broadcast_status(status_tx, &guard);
    }

    // Create progress watch channel and spawn the download
    let (progress_tx, mut progress_rx) = watch::channel(DownloadProgress {
        bytes_downloaded: 0,
        total_bytes: 0,
        percent: 0.0,
    });

    let download_url = pending.download_url.clone();
    let download_future = updater.download_update_with_progress(&download_url, progress_tx);
    tokio::pin!(download_future);

    // Forward progress to broadcast subscribers via periodic polling
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(250));
    tick.tick().await; // consume immediate first tick

    let result = loop {
        tokio::select! {
            biased;
            result = &mut download_future => {
                // Download finished — push final progress snapshot
                let final_progress = progress_rx.borrow().clone();
                {
                    let mut guard = state.write().await;
                    guard.download_progress = Some(final_progress);
                    guard.touch();
                    broadcast_status(status_tx, &guard);
                }
                break result;
            }
            _ = tick.tick() => {
                if progress_rx.has_changed().unwrap_or(false) {
                    progress_rx.mark_changed();
                    let snap = progress_rx.borrow_and_update().clone();
                    let mut guard = state.write().await;
                    guard.download_progress = Some(snap);
                    guard.touch();
                    broadcast_status(status_tx, &guard);
                }
            }
        }
    };

    match result {
        Ok(path) => {
            info!("Download completed: {:?}", path);
            *downloaded_path = Some(path);
            let mut guard = state.write().await;
            guard.phase = UpdatePhase::ReadyToInstall;
            guard.message = Some(format!(
                "Download complete. Ready to install version {}",
                pending.latest_version
            ));
            guard.download_progress = None;
            guard.touch();
            broadcast_status(status_tx, &guard);
            Ok(())
        }
        Err(e) => {
            *downloaded_path = None;
            Err(e)
        }
    }
}

/// Phase 2: Install a previously downloaded update.
async fn run_install<E: UpdateExecutor>(
    updater: &E,
    state: &Arc<RwLock<UpdateStatus>>,
    status_tx: Option<&broadcast::Sender<UpdateStatus>>,
    downloaded_path: &mut Option<PathBuf>,
) -> Result<(), UpdateError> {
    let path = match downloaded_path.take() {
        Some(p) => p,
        None => {
            return Err(UpdateError::Install(
                "No downloaded file available for installation".to_string(),
            ));
        }
    };

    let version_label = {
        let guard = state.read().await;
        guard
            .pending
            .as_ref()
            .map(|p| p.latest_version.clone())
            .unwrap_or_else(|| "unknown".to_string())
    };

    // Transition to Installing
    {
        let mut guard = state.write().await;
        guard.phase = UpdatePhase::Installing;
        guard.message = Some(format!("Installing version {}", version_label));
        guard.download_progress = None;
        guard.touch();
        broadcast_status(status_tx, &guard);
    }

    match updater.install_and_restart(&path) {
        Ok(()) => {
            let mut guard = state.write().await;
            guard.phase = UpdatePhase::Updated;
            guard.message = Some(format!("Updated to version {}", version_label));
            guard.pending = None;
            guard.download_progress = None;
            guard.touch();
            broadcast_status(status_tx, &guard);
            Ok(())
        }
        Err(e) => {
            error!("Update installation failed: {}", e);
            let mut guard = state.write().await;
            guard.phase = UpdatePhase::Error;
            guard.message = Some(format!("Update installation failed: {}", e));
            guard.download_progress = None;
            guard.touch();
            broadcast_status(status_tx, &guard);
            Err(e)
        }
    }
}

/// Set the coordinator state to Error with a message.
async fn emit_error(
    state: &Arc<RwLock<UpdateStatus>>,
    status_tx: Option<&broadcast::Sender<UpdateStatus>>,
    message: &str,
) {
    let mut guard = state.write().await;
    guard.phase = UpdatePhase::Error;
    guard.message = Some(message.to_string());
    guard.download_progress = None;
    guard.touch();
    broadcast_status(status_tx, &guard);
}

/// Send an UpdateStatus snapshot to broadcast subscribers (if any).
fn broadcast_status(status_tx: Option<&broadcast::Sender<UpdateStatus>>, status: &UpdateStatus) {
    if let Some(tx) = status_tx {
        if let Err(e) = tx.send(status.clone()) {
            debug!("channel send failed: {e}");
        }
    }
}

async fn run_check<E: UpdateExecutor>(
    updater: &E,
    state: &Arc<RwLock<UpdateStatus>>,
    status_tx: Option<&broadcast::Sender<UpdateStatus>>,
    auto_install: bool,
    downloaded_path: &mut Option<PathBuf>,
) {
    {
        let mut guard = state.write().await;
        guard.phase = UpdatePhase::Checking;
        guard.message = Some("Checking for new version".to_string());
        guard.pending = None;
        guard.download_progress = None;
        guard.touch();
        broadcast_status(status_tx, &guard);
    }

    let result = updater.check_for_updates().await;
    match result {
        Ok(UpdateCheckResult::UpToDate { current }) => {
            let mut guard = state.write().await;
            guard.phase = UpdatePhase::Idle;
            guard.message = Some(format!("Already on latest version: {}", current));
            guard.pending = None;
            guard.touch();
            broadcast_status(status_tx, &guard);
        }
        Ok(UpdateCheckResult::Available {
            current,
            latest,
            release,
            download_url,
            download_size,
            ..
        }) => {
            {
                let mut guard = state.write().await;
                guard.phase = UpdatePhase::PendingApproval;
                guard.message = Some(format!("New version detected: {} -> {}", current, latest));
                guard.pending = Some(PendingUpdateInfo {
                    current_version: current.to_string(),
                    latest_version: latest.to_string(),
                    release_url: release.html_url.clone(),
                    release_name: release.name.clone(),
                    published_at: release.published_at.clone(),
                    download_url,
                    release_notes: release.body.clone(),
                    download_size_bytes: download_size,
                });
                guard.touch();
                broadcast_status(status_tx, &guard);
            }

            if auto_install {
                info!("Auto-update mode enabled: downloading and installing");
                // Phase 1: Download
                if let Err(e) = run_download(updater, state, status_tx, downloaded_path).await {
                    emit_error(state, status_tx, &format!("Auto-download failed: {e}")).await;
                    return;
                }
                // Phase 2: Install
                if let Err(e) = run_install(updater, state, status_tx, downloaded_path).await {
                    emit_error(state, status_tx, &format!("Auto-install failed: {e}")).await;
                }
            }
        }
        Err(UpdateError::Disabled) => {
            let mut guard = state.write().await;
            guard.phase = UpdatePhase::Idle;
            guard.message = Some("Update feature is disabled".to_string());
            guard.pending = None;
            guard.touch();
            broadcast_status(status_tx, &guard);
        }
        Err(e) => {
            warn!("Failed to check for updates: {}", e);
            let mut guard = state.write().await;
            guard.phase = UpdatePhase::Error;
            guard.message = Some(format!("Failed to check for updates: {}", e));
            guard.pending = None;
            guard.touch();
            broadcast_status(status_tx, &guard);
        }
    }

    if let Err(e) = updater.save_last_check_time() {
        warn!("Failed to persist last update check timestamp: {}", e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::updater::{ReleaseAsset, ReleaseInfo, UpdateAssetType};
    use std::collections::VecDeque;
    use std::sync::Mutex as StdMutex;
    use tokio::sync::Mutex;

    #[derive(Clone)]
    struct FakeUpdater {
        should_check: bool,
        check_results: Arc<Mutex<VecDeque<Result<UpdateCheckResult, UpdateError>>>>,
        install_error: Arc<StdMutex<Option<String>>>,
        download_error: Arc<StdMutex<Option<String>>>,
    }

    impl FakeUpdater {
        fn with_result(result: Result<UpdateCheckResult, UpdateError>) -> Self {
            Self {
                should_check: false,
                check_results: Arc::new(Mutex::new(VecDeque::from([result]))),
                install_error: Arc::new(StdMutex::new(None)),
                download_error: Arc::new(StdMutex::new(None)),
            }
        }

        fn set_install_error(&self, err: &str) {
            *self
                .install_error
                .lock()
                .expect("install_error mutex poisoned") = Some(err.to_string());
        }

        fn set_download_error(&self, err: &str) {
            *self
                .download_error
                .lock()
                .expect("download_error mutex poisoned") = Some(err.to_string());
        }
    }

    #[async_trait]
    impl UpdateExecutor for FakeUpdater {
        fn should_check_for_updates(&self) -> bool {
            self.should_check
        }

        async fn check_for_updates(&self) -> Result<UpdateCheckResult, UpdateError> {
            self.check_results
                .lock()
                .await
                .pop_front()
                .unwrap_or_else(|| {
                    Ok(UpdateCheckResult::UpToDate {
                        current: semver::Version::parse("0.0.1").expect("valid test semver"),
                    })
                })
        }

        fn save_last_check_time(&self) -> Result<(), UpdateError> {
            Ok(())
        }

        async fn download_update(&self, _download_url: &str) -> Result<PathBuf, UpdateError> {
            if let Some(err) = self
                .download_error
                .lock()
                .expect("download_error mutex poisoned")
                .clone()
            {
                return Err(UpdateError::Download(err));
            }
            Ok(PathBuf::from("/tmp/oneshim-test-update.tar.gz"))
        }

        async fn download_update_with_progress(
            &self,
            download_url: &str,
            progress_tx: watch::Sender<DownloadProgress>,
        ) -> Result<PathBuf, UpdateError> {
            // Simulate progress steps
            let _ = progress_tx.send(DownloadProgress {
                bytes_downloaded: 50,
                total_bytes: 100,
                percent: 50.0,
            });
            let _ = progress_tx.send(DownloadProgress {
                bytes_downloaded: 100,
                total_bytes: 100,
                percent: 100.0,
            });
            self.download_update(download_url).await
        }

        fn install_and_restart(&self, _downloaded_path: &Path) -> Result<(), UpdateError> {
            if let Some(err) = self
                .install_error
                .lock()
                .expect("install_error mutex poisoned")
                .clone()
            {
                return Err(UpdateError::Install(err));
            }
            Ok(())
        }
    }

    fn make_available_result(
        current: &str,
        latest: &str,
    ) -> Result<UpdateCheckResult, UpdateError> {
        Ok(UpdateCheckResult::Available {
            current: semver::Version::parse(current).expect("valid current version"),
            latest: semver::Version::parse(latest).expect("valid latest version"),
            release: Box::new(ReleaseInfo {
                tag_name: format!("v{}", latest),
                name: Some(format!("Release {}", latest)),
                body: None,
                prerelease: false,
                assets: vec![ReleaseAsset {
                    name: "oneshim-macos-arm64.tar.gz".to_string(),
                    browser_download_url:
                        "https://github.com/pseudotop/maekon-client/releases/download/v1.2.0/oneshim-macos-arm64.tar.gz"
                            .to_string(),
                    size: 123,
                    content_type: "application/gzip".to_string(),
                }],
                html_url: "https://github.com/pseudotop/maekon-client/releases/v1.2.0".to_string(),
                published_at: Some("2026-02-21T10:00:00Z".to_string()),
            }),
            download_url:
                "https://github.com/pseudotop/maekon-client/releases/download/v1.2.0/oneshim-macos-arm64.tar.gz"
                    .to_string(),
            download_size: None,
            asset_type: UpdateAssetType::FullBinary,
        })
    }

    /// Two-phase flow: Check → Approve (downloads) → Approve (installs) → Updated
    #[tokio::test]
    async fn e2e_two_phase_approve_download_then_install() {
        let fake = FakeUpdater::with_result(make_available_result("1.0.0", "1.2.0"));
        let state = Arc::new(RwLock::new(UpdateStatus::default()));
        let (status_tx, mut status_rx) = broadcast::channel::<UpdateStatus>(64);
        let (tx, rx) = mpsc::unbounded_channel();

        let coordinator = tokio::spawn(run_update_coordinator_with_executor(
            fake,
            state.clone(),
            rx,
            Some(status_tx),
            false,
            24,
        ));

        tx.send(UpdateAction::CheckNow).expect("send check");

        // Wait for PendingApproval
        loop {
            if let Ok(s) = status_rx.recv().await {
                if s.phase == UpdatePhase::PendingApproval {
                    break;
                }
            }
        }

        // First Approve → triggers download → ReadyToInstall
        tx.send(UpdateAction::Approve)
            .expect("send approve for download");
        loop {
            if let Ok(s) = status_rx.recv().await {
                if s.phase == UpdatePhase::ReadyToInstall {
                    break;
                }
            }
        }

        // Second Approve → triggers install → Updated
        tx.send(UpdateAction::Approve)
            .expect("send approve for install");
        loop {
            if let Ok(s) = status_rx.recv().await {
                if s.phase == UpdatePhase::Updated {
                    break;
                }
            }
        }

        drop(tx);
        coordinator.await.expect("join coordinator task");

        let final_state = state.read().await.clone();
        assert_eq!(final_state.phase, UpdatePhase::Updated);
        assert!(final_state.pending.is_none());
        assert!(final_state.download_progress.is_none());
    }

    /// Auto-install: Check → auto-download → auto-install → Updated
    #[tokio::test]
    async fn e2e_auto_install_runs_both_phases() {
        let fake = FakeUpdater::with_result(make_available_result("1.0.0", "1.2.0"));
        let state = Arc::new(RwLock::new(UpdateStatus::default()));
        let (tx, rx) = mpsc::unbounded_channel();

        let coordinator = tokio::spawn(run_update_coordinator_with_executor(
            fake,
            state.clone(),
            rx,
            None,
            true, // auto_install
            24,
        ));

        tx.send(UpdateAction::CheckNow).expect("send check");
        drop(tx);
        coordinator.await.expect("join coordinator task");

        let final_state = state.read().await.clone();
        assert_eq!(final_state.phase, UpdatePhase::Updated);
        assert!(final_state.pending.is_none());
    }

    /// Install failure after successful download sets Error phase
    #[tokio::test]
    async fn e2e_install_failure_after_download_sets_error() {
        let fake = FakeUpdater::with_result(make_available_result("1.0.0", "1.2.0"));
        fake.set_install_error("simulated restart failure");
        let state = Arc::new(RwLock::new(UpdateStatus::default()));
        let (status_tx, mut status_rx) = broadcast::channel::<UpdateStatus>(64);
        let (tx, rx) = mpsc::unbounded_channel();

        let coordinator = tokio::spawn(run_update_coordinator_with_executor(
            fake,
            state.clone(),
            rx,
            Some(status_tx),
            false,
            24,
        ));

        tx.send(UpdateAction::CheckNow).expect("send check");
        loop {
            if let Ok(s) = status_rx.recv().await {
                if s.phase == UpdatePhase::PendingApproval {
                    break;
                }
            }
        }

        tx.send(UpdateAction::Approve)
            .expect("send approve download");
        loop {
            if let Ok(s) = status_rx.recv().await {
                if s.phase == UpdatePhase::ReadyToInstall {
                    break;
                }
            }
        }

        tx.send(UpdateAction::Approve)
            .expect("send approve install");
        loop {
            if let Ok(s) = status_rx.recv().await {
                if s.phase == UpdatePhase::Error {
                    break;
                }
            }
        }

        drop(tx);
        coordinator.await.expect("join coordinator task");

        let final_state = state.read().await.clone();
        assert_eq!(final_state.phase, UpdatePhase::Error);
        assert!(final_state
            .message
            .expect("message")
            .to_lowercase()
            .contains("failed"));
    }

    /// Download failure sets Error phase (never reaches ReadyToInstall)
    #[tokio::test]
    async fn e2e_download_failure_sets_error() {
        let fake = FakeUpdater::with_result(make_available_result("1.0.0", "1.2.0"));
        fake.set_download_error("network timeout");
        let state = Arc::new(RwLock::new(UpdateStatus::default()));
        let (status_tx, mut status_rx) = broadcast::channel::<UpdateStatus>(64);
        let (tx, rx) = mpsc::unbounded_channel();

        let coordinator = tokio::spawn(run_update_coordinator_with_executor(
            fake,
            state.clone(),
            rx,
            Some(status_tx),
            false,
            24,
        ));

        tx.send(UpdateAction::CheckNow).expect("send check");
        loop {
            if let Ok(s) = status_rx.recv().await {
                if s.phase == UpdatePhase::PendingApproval {
                    break;
                }
            }
        }

        tx.send(UpdateAction::Approve)
            .expect("send approve download");
        loop {
            if let Ok(s) = status_rx.recv().await {
                if s.phase == UpdatePhase::Error {
                    break;
                }
            }
        }

        drop(tx);
        coordinator.await.expect("join coordinator task");

        let final_state = state.read().await.clone();
        assert_eq!(final_state.phase, UpdatePhase::Error);
        assert!(final_state
            .message
            .expect("message")
            .to_lowercase()
            .contains("download"));
    }

    #[tokio::test]
    async fn e2e_update_flow_defer_after_detect() {
        let fake = FakeUpdater::with_result(make_available_result("1.0.0", "1.2.0"));
        let state = Arc::new(RwLock::new(UpdateStatus::default()));
        let (tx, rx) = mpsc::unbounded_channel();

        let coordinator = tokio::spawn(run_update_coordinator_with_executor(
            fake,
            state.clone(),
            rx,
            None,
            false,
            24,
        ));

        tx.send(UpdateAction::CheckNow).expect("send check action");
        tx.send(UpdateAction::Defer).expect("send defer action");
        drop(tx);
        coordinator.await.expect("join coordinator task");

        let final_state = state.read().await.clone();
        assert_eq!(final_state.phase, UpdatePhase::Deferred);
        assert!(final_state.pending.is_none());
    }

    #[tokio::test]
    async fn defer_is_ignored_when_update_is_not_actionable() {
        let fake = FakeUpdater::with_result(make_available_result("1.0.0", "1.2.0"));
        let state = Arc::new(RwLock::new(UpdateStatus::default()));
        let (tx, rx) = mpsc::unbounded_channel();

        let coordinator = tokio::spawn(run_update_coordinator_with_executor(
            fake,
            state.clone(),
            rx,
            None,
            false,
            24,
        ));

        tx.send(UpdateAction::Defer).expect("send defer action");
        drop(tx);
        coordinator.await.expect("join coordinator task");

        let final_state = state.read().await.clone();
        assert_eq!(final_state.phase, UpdatePhase::Idle);
        assert!(final_state.pending.is_none());
    }

    /// Progress is broadcast during the Downloading phase
    #[tokio::test]
    async fn e2e_download_progress_is_broadcast() {
        let fake = FakeUpdater::with_result(make_available_result("1.0.0", "1.2.0"));
        let state = Arc::new(RwLock::new(UpdateStatus::default()));
        let (status_tx, mut status_rx) = broadcast::channel::<UpdateStatus>(64);
        let (tx, rx) = mpsc::unbounded_channel();

        let coordinator = tokio::spawn(run_update_coordinator_with_executor(
            fake,
            state.clone(),
            rx,
            Some(status_tx),
            false,
            24,
        ));

        tx.send(UpdateAction::CheckNow).expect("send check");
        loop {
            if let Ok(s) = status_rx.recv().await {
                if s.phase == UpdatePhase::PendingApproval {
                    break;
                }
            }
        }

        tx.send(UpdateAction::Approve).expect("send approve");

        // Collect status events until ReadyToInstall
        let mut saw_downloading = false;
        loop {
            if let Ok(s) = status_rx.recv().await {
                if s.phase == UpdatePhase::Downloading {
                    saw_downloading = true;
                }
                if s.phase == UpdatePhase::ReadyToInstall {
                    break;
                }
            }
        }
        assert!(
            saw_downloading,
            "expected Downloading phase to be broadcast"
        );

        drop(tx);
        coordinator.await.expect("join coordinator task");
    }
}
