use super::executor::UpdateExecutor;
use super::run_update_coordinator_with_executor;
use crate::updater::{ReleaseAsset, ReleaseInfo, UpdateAssetType, UpdateCheckResult, UpdateError};
use async_trait::async_trait;
use maekon_api_contracts::update::DownloadProgress;
use maekon_web::update_control::{UpdateAction, UpdatePhase, UpdateStatus};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use tokio::sync::{broadcast, mpsc, watch, Mutex, RwLock};

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
        Ok(PathBuf::from("/tmp/maekon-test-update.tar.gz"))
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

fn make_available_result(current: &str, latest: &str) -> Result<UpdateCheckResult, UpdateError> {
    Ok(UpdateCheckResult::Available {
        current: semver::Version::parse(current).expect("valid current version"),
        latest: semver::Version::parse(latest).expect("valid latest version"),
        release: Box::new(ReleaseInfo {
            tag_name: format!("v{}", latest),
            name: Some(format!("Release {}", latest)),
            body: None,
            prerelease: false,
            assets: vec![ReleaseAsset {
                name: "maekon-macos-arm64.tar.gz".to_string(),
                browser_download_url:
                    "https://github.com/pseudotop/maekon-client/releases/download/v1.2.0/maekon-macos-arm64.tar.gz"
                        .to_string(),
                size: 123,
                content_type: "application/gzip".to_string(),
            }],
            html_url: "https://github.com/pseudotop/maekon-client/releases/v1.2.0".to_string(),
            published_at: Some("2026-02-21T10:00:00Z".to_string()),
        }),
        download_url:
            "https://github.com/pseudotop/maekon-client/releases/download/v1.2.0/maekon-macos-arm64.tar.gz"
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
