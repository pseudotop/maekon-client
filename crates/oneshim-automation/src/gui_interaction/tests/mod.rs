use super::*;
use async_trait::async_trait;
use oneshim_core::models::gui::{FocusSnapshot, FocusValidation, GuiActionType, HighlightHandle};
use oneshim_core::models::intent::{ElementBounds, UiElement};
use oneshim_core::models::ui_scene::{NormalizedBounds, UiScene, UiSceneElement};
use std::sync::atomic::AtomicUsize;
use std::sync::Mutex;

mod confirm;
mod execute;
mod highlight;
mod m5;
mod session;

// ── Test constants ──────────────────────────────────────────────────

const TEST_HMAC_SECRET: &str = "test-hmac-secret-32-bytes-long!!";

// ── MockElementFinder ───────────────────────────────────────────────

struct MockElementFinder {
    scene: Mutex<UiScene>,
}

impl MockElementFinder {
    fn new(scene: UiScene) -> Self {
        Self {
            scene: Mutex::new(scene),
        }
    }
}

#[async_trait]
impl ElementFinder for MockElementFinder {
    async fn find_element(
        &self,
        _text: Option<&str>,
        _role: Option<&str>,
        _region: Option<&ElementBounds>,
    ) -> Result<Vec<UiElement>, CoreError> {
        Ok(vec![])
    }

    async fn analyze_scene(
        &self,
        _app_name: Option<&str>,
        _screen_id: Option<&str>,
    ) -> Result<UiScene, CoreError> {
        Ok(self.scene.lock().unwrap().clone())
    }

    fn name(&self) -> &str {
        "mock"
    }
}

// ── MockFocusProbe ──────────────────────────────────────────────────

struct MockFocusProbe {
    focus: Mutex<FocusSnapshot>,
    validation_valid: Mutex<bool>,
    /// When set, validation returns invalid for first N calls, then valid.
    drift_recover_after: Mutex<Option<usize>>,
    validation_call_count: AtomicUsize,
}

impl MockFocusProbe {
    fn new(focus: FocusSnapshot) -> Self {
        Self {
            focus: Mutex::new(focus),
            validation_valid: Mutex::new(true),
            drift_recover_after: Mutex::new(None),
            validation_call_count: AtomicUsize::new(0),
        }
    }

    fn set_validation_valid(&self, valid: bool) {
        *self.validation_valid.lock().unwrap() = valid;
    }

    /// Focus returns invalid for first `n` calls, then valid.
    fn set_drift_recover_after(&self, n: usize) {
        *self.drift_recover_after.lock().unwrap() = Some(n);
    }
}

#[async_trait]
impl FocusProbe for MockFocusProbe {
    async fn current_focus(&self) -> Result<FocusSnapshot, CoreError> {
        Ok(self.focus.lock().unwrap().clone())
    }

    async fn validate_execution_binding(
        &self,
        _binding: &ExecutionBinding,
    ) -> Result<FocusValidation, CoreError> {
        let call_num = self.validation_call_count.fetch_add(1, Ordering::SeqCst);

        let valid = if let Some(recover_after) = *self.drift_recover_after.lock().unwrap() {
            call_num >= recover_after
        } else {
            *self.validation_valid.lock().unwrap()
        };

        Ok(FocusValidation {
            valid,
            reason: if valid {
                None
            } else {
                Some("Focus changed".to_string())
            },
            current_focus: None,
        })
    }
}

// ── MockOverlayDriver ───────────────────────────────────────────────

struct MockOverlayDriver {
    show_count: AtomicUsize,
    clear_count: AtomicUsize,
}

impl MockOverlayDriver {
    fn new() -> Self {
        Self {
            show_count: AtomicUsize::new(0),
            clear_count: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl OverlayDriver for MockOverlayDriver {
    async fn show_highlights(&self, req: HighlightRequest) -> Result<HighlightHandle, CoreError> {
        self.show_count.fetch_add(1, Ordering::SeqCst);
        Ok(HighlightHandle {
            handle_id: format!("handle-{}", self.show_count.load(Ordering::SeqCst)),
            rendered_at: Utc::now(),
            target_count: req.targets.len(),
        })
    }

    async fn clear_highlights(&self, _handle_id: &str) -> Result<(), CoreError> {
        self.clear_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn show_detection(
        &self,
        _scene: &oneshim_core::models::ui_scene::UiScene,
    ) -> Result<(), oneshim_core::error::CoreError> {
        Ok(())
    }

    async fn clear_detection(&self) -> Result<(), oneshim_core::error::CoreError> {
        Ok(())
    }
}

// ── PermissionDeniedElementFinder ────────────────────────────────────

struct PermissionDeniedElementFinder;

#[async_trait]
impl ElementFinder for PermissionDeniedElementFinder {
    async fn find_element(
        &self,
        _text: Option<&str>,
        _role: Option<&str>,
        _region: Option<&ElementBounds>,
    ) -> Result<Vec<UiElement>, CoreError> {
        Err(CoreError::PolicyDenied {
            code: oneshim_core::error_codes::PolicyCode::Denied,
            message: "Accessibility permission denied".to_string(),
        })
    }

    async fn analyze_scene(
        &self,
        _app_name: Option<&str>,
        _screen_id: Option<&str>,
    ) -> Result<UiScene, CoreError> {
        Err(CoreError::PolicyDenied {
            code: oneshim_core::error_codes::PolicyCode::Denied,
            message: "Accessibility permission denied".to_string(),
        })
    }

    fn name(&self) -> &str {
        "permission-denied-mock"
    }
}

// ── Fixture builders ────────────────────────────────────────────────

fn make_element(id: &str, label: &str, confidence: f64) -> UiSceneElement {
    UiSceneElement {
        element_id: id.to_string(),
        bbox_abs: ElementBounds {
            x: 100,
            y: 80,
            width: 200,
            height: 40,
        },
        bbox_norm: NormalizedBounds::new(0.05, 0.07, 0.10, 0.04),
        label: label.to_string(),
        role: Some("button".to_string()),
        intent: None,
        state: Some("enabled".to_string()),
        confidence,
        text_masked: Some(label.to_string()),
        parent_id: None,
    }
}

fn make_scene(elements: Vec<UiSceneElement>) -> UiScene {
    UiScene {
        schema_version: "ui_scene.v1".to_string(),
        scene_id: "test-scene-1".to_string(),
        app_name: Some("TestApp".to_string()),
        screen_id: Some("screen-main".to_string()),
        captured_at: Utc::now(),
        screen_width: 1920,
        screen_height: 1080,
        elements,
    }
}

fn make_focus() -> FocusSnapshot {
    FocusSnapshot {
        app_name: "TestApp".to_string(),
        window_title: "Test Window".to_string(),
        pid: 1234,
        bounds: None,
        captured_at: Utc::now(),
        focus_hash: "abc123hash".to_string(),
    }
}

fn make_service(
    scene: UiScene,
    focus: FocusSnapshot,
) -> (Arc<GuiInteractionService>, Arc<MockFocusProbe>) {
    let (service, probe, _) = make_service_full(scene, focus);
    (service, probe)
}

fn make_service_full(
    scene: UiScene,
    focus: FocusSnapshot,
) -> (
    Arc<GuiInteractionService>,
    Arc<MockFocusProbe>,
    Arc<MockOverlayDriver>,
) {
    let probe = Arc::new(MockFocusProbe::new(focus));
    let overlay = Arc::new(MockOverlayDriver::new());
    let service = Arc::new(GuiInteractionService::new(
        Arc::new(MockElementFinder::new(scene)),
        probe.clone(),
        overlay.clone(),
        Some(TEST_HMAC_SECRET.to_string()),
    ));
    (service, probe, overlay)
}

fn make_service_with_finder(
    finder: Arc<dyn ElementFinder>,
    focus: FocusSnapshot,
) -> Arc<GuiInteractionService> {
    Arc::new(GuiInteractionService::new(
        finder,
        Arc::new(MockFocusProbe::new(focus)),
        Arc::new(MockOverlayDriver::new()),
        Some(TEST_HMAC_SECRET.to_string()),
    ))
}

#[allow(dead_code)]
fn make_drifted_focus() -> FocusSnapshot {
    FocusSnapshot {
        app_name: "OtherApp".to_string(),
        window_title: "Other Window".to_string(),
        pid: 9999,
        bounds: None,
        captured_at: Utc::now(),
        focus_hash: "drifted-hash-999".to_string(),
    }
}

fn default_create_request() -> GuiCreateSessionRequest {
    GuiCreateSessionRequest {
        app_name: Some("TestApp".to_string()),
        screen_id: None,
        min_confidence: None,
        max_candidates: None,
        session_ttl_secs: Some(300),
    }
}

/// Helper: create a session and return (session_id, capability_token)
async fn create_test_session(service: &GuiInteractionService) -> (String, String) {
    let resp = service
        .create_session(default_create_request())
        .await
        .expect("create_session should succeed");
    (resp.session.session_id, resp.capability_token)
}

/// Helper: create session + highlight it, returns (session_id, token)
async fn create_and_highlight(service: &GuiInteractionService) -> (String, String) {
    let (sid, token) = create_test_session(service).await;
    service
        .highlight_session(
            &sid,
            &token,
            GuiHighlightRequest {
                candidate_ids: None,
            },
        )
        .await
        .expect("highlight should succeed");
    (sid, token)
}

/// Helper: create session + highlight + confirm, returns (session_id, token, ticket)
async fn create_highlight_and_confirm(
    service: &GuiInteractionService,
) -> (String, String, GuiExecutionTicket) {
    let (sid, token) = create_and_highlight(service).await;

    let session = service.get_session(&sid, &token).await.unwrap();
    let candidate_id = session.candidates[0].element.element_id.clone();

    let ticket = service
        .confirm_candidate(
            &sid,
            &token,
            GuiConfirmRequest {
                candidate_id,
                action: GuiActionRequest {
                    action_type: GuiActionType::Click,
                    text: None,
                },
                ticket_ttl_secs: Some(60),
            },
        )
        .await
        .expect("confirm should succeed");
    (sid, token, ticket)
}
