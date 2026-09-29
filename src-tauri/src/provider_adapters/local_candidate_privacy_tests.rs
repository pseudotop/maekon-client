use super::ExternalOcrPrivacyGuard;
use async_trait::async_trait;
use maekon_core::config::{ExternalDataPolicy, PiiFilterLevel, PrivacyConfig};
use maekon_core::consent::{ConsentManager, ConsentPermissions};
use maekon_core::error::CoreError;
use maekon_core::models::candidate_decision::{window_digest, DecisionUnavailable};
use maekon_core::models::context::{ProcessInfo, WindowInfo};
use maekon_core::models::event::ProcessDetail;
use maekon_core::ports::consent_manager::ConsentManagerPort;
use maekon_core::ports::monitor::ProcessMonitor;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

type Hook = Box<dyn FnOnce() + Send>;
struct Monitor {
    window: Mutex<Option<WindowInfo>>,
    fail: AtomicBool,
    calls: AtomicUsize,
    after_read: Mutex<Option<Hook>>,
}
#[async_trait]
impl ProcessMonitor for Monitor {
    async fn get_active_window(&self) -> Result<Option<WindowInfo>, CoreError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail.load(Ordering::SeqCst) {
            return Err(CoreError::PolicyDenied {
                code: maekon_core::error_codes::PolicyCode::Denied,
                message: "Test-only window observation failure".into(),
            });
        }
        let window = self.window.lock().clone();
        tokio::task::yield_now().await;
        let hook = self.after_read.lock().take();
        if let Some(hook) = hook {
            hook();
        }
        Ok(window)
    }
    async fn get_top_processes(&self, _limit: usize) -> Result<Vec<ProcessInfo>, CoreError> {
        Ok(vec![])
    }
    async fn get_detailed_processes(
        &self,
        _foreground_pid: Option<u32>,
        _top_n: usize,
    ) -> Result<Vec<ProcessDetail>, CoreError> {
        Ok(vec![])
    }
}
struct Fixture {
    guard: ExternalOcrPrivacyGuard,
    consent: Arc<ConsentManager>,
    monitor: Arc<Monitor>,
    _directory: tempfile::TempDir,
}
fn permissions() -> ConsentPermissions {
    // Existing snapshot assessment does not initiate an OCR operation.
    ConsentPermissions {
        full_text_extraction: true,
        ..Default::default()
    }
}
fn fixture(excluded: bool) -> Fixture {
    let directory = tempfile::TempDir::new().unwrap();
    let consent = Arc::new(ConsentManager::new(directory.path().join("consent.json")));
    consent.grant_consent(permissions(), 30).unwrap();
    let monitor = Arc::new(Monitor {
        window: Mutex::new(Some(WindowInfo {
            title: "Editor".into(),
            app_name: "Editor".into(),
            app_bundle_id: None,
            pid: 1,
            bounds: None,
        })),
        fail: AtomicBool::new(false),
        calls: AtomicUsize::new(0),
        after_read: Mutex::new(None),
    });
    let policy = PrivacyConfig {
        // Exercise the hard sensitive-app gate independently of policy exclusion.
        auto_exclude_sensitive: false,
        excluded_apps: if excluded {
            vec!["Editor".into()]
        } else {
            vec![]
        },
        ..Default::default()
    };
    let guard = ExternalOcrPrivacyGuard::new(
        consent.clone(),
        PiiFilterLevel::Strict,
        ExternalDataPolicy::PiiFilterStrict,
        policy,
        monitor.clone(),
        None,
    );
    Fixture {
        guard,
        consent,
        monitor,
        _directory: directory,
    }
}

#[tokio::test]
async fn local_candidate_privacy_returns_exact_revision_and_observed_window() {
    let f = fixture(false);
    let revision = f.guard.candidate_consent_revision().unwrap();
    let window = f.monitor.window.lock().clone().unwrap();
    assert!(!f.consent.ocr_processing_permitted());
    assert_eq!(
        f.guard.local_candidate_authority().await,
        Ok((revision, window_digest(&window).unwrap()))
    );
    assert_eq!(f.monitor.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn local_candidate_privacy_denies_revocation_erasure_and_missing_permission_before_probe() {
    for case in 0..4 {
        let f = fixture(false);
        match case {
            0 => f
                .consent
                .grant_consent(ConsentPermissions::default(), 30)
                .unwrap(),
            1 => f.consent.erasing().store(true, Ordering::SeqCst),
            2 => f.consent.revoke_consent().unwrap(),
            _ => {
                f.consent.revoke_consent().unwrap();
                f.consent.grant_consent(permissions(), 30).unwrap();
                assert!(f.consent.has_pending_deletion());
            }
        }
        assert_eq!(
            f.guard.local_candidate_authority().await,
            Err(DecisionUnavailable::ConsentOrPolicyDenied)
        );
        assert_eq!(f.monitor.calls.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn local_candidate_privacy_denies_sensitive_excluded_absent_and_failed_observations() {
    for case in 0..4 {
        let f = fixture(case == 1);
        match case {
            0 => f.monitor.window.lock().as_mut().unwrap().app_name = "1Password".into(),
            1 => {}
            2 => *f.monitor.window.lock() = None,
            _ => f.monitor.fail.store(true, Ordering::SeqCst),
        }
        assert_eq!(
            f.guard.local_candidate_authority().await,
            Err(DecisionUnavailable::ConsentOrPolicyDenied)
        );
        assert_eq!(f.monitor.calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn local_candidate_privacy_rechecks_still_valid_consent_after_async_observation() {
    let f = fixture(false);
    let consent = f.consent.clone();
    *f.monitor.after_read.lock() = Some(Box::new(move || {
        consent
            .grant_consent(
                ConsentPermissions {
                    microphone: true,
                    ..permissions()
                },
                30,
            )
            .unwrap();
    }));
    assert_eq!(
        f.guard.local_candidate_authority().await,
        Err(DecisionUnavailable::ConsentOrPolicyDenied)
    );
    assert!(f.consent.full_text_extraction_permitted());
    assert_eq!(f.monitor.calls.load(Ordering::SeqCst), 1);
}
