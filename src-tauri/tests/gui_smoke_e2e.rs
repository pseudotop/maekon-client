//! E2E smoke tests for the GUI V2 interaction flow (ADR-002).
//!
//! These tests wire mock adapters through the same DI chain that the Tauri
//! binary uses, exercising the full propose → highlight → confirm → execute
//! lifecycle plus failure scenarios.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::Utc;

use oneshim_automation::gui_interaction::GuiInteractionService;
use oneshim_core::error::{CoreError, GuiInteractionError};
use oneshim_core::models::gui::*;
use oneshim_core::models::intent::ElementBounds;
use oneshim_core::models::ui_scene::{NormalizedBounds, UiScene, UiSceneElement};
use oneshim_core::ports::element_finder::ElementFinder;
use oneshim_core::ports::focus_probe::FocusProbe;
use oneshim_core::ports::overlay_driver::OverlayDriver;

const TEST_HMAC_SECRET: &str = "e2e-smoke-test-secret-key-2026";

// ── Mock Adapters ───────────────────────────────────────────────────

struct E2eFinder {
    should_fail: AtomicBool,
    fail_permission: AtomicBool,
}

impl E2eFinder {
    fn new() -> Self {
        Self {
            should_fail: AtomicBool::new(false),
            fail_permission: AtomicBool::new(false),
        }
    }

    fn scene() -> UiScene {
        UiScene {
            schema_version: "ui_scene.v1".into(),
            scene_id: "e2e-scene-001".into(),
            app_name: Some("TestApp".into()),
            screen_id: None,
            captured_at: Utc::now(),
            screen_width: 1920,
            screen_height: 1080,
            elements: vec![UiSceneElement {
                element_id: "btn-save".into(),
                bbox_abs: ElementBounds {
                    x: 100,
                    y: 50,
                    width: 80,
                    height: 30,
                },
                bbox_norm: NormalizedBounds {
                    x: 0.05,
                    y: 0.05,
                    width: 0.04,
                    height: 0.03,
                },
                label: "Save".into(),
                role: Some("button".into()),
                intent: None,
                state: None,
                confidence: 0.95,
                text_masked: Some("Save".into()),
                parent_id: None,
            }],
        }
    }
}

#[async_trait]
impl ElementFinder for E2eFinder {
    async fn find_element(
        &self,
        _: Option<&str>,
        _: Option<&str>,
        _: Option<&ElementBounds>,
    ) -> Result<Vec<oneshim_core::models::intent::UiElement>, CoreError> {
        Ok(vec![])
    }

    async fn analyze_scene(&self, _: Option<&str>, _: Option<&str>) -> Result<UiScene, CoreError> {
        if self.fail_permission.load(Ordering::Relaxed) {
            return Err(CoreError::PermissionDenied {
                code: oneshim_core::error_codes::PermissionCode::PermissionDenied,
                message: "Accessibility denied".into(),
            });
        }
        if self.should_fail.load(Ordering::Relaxed) {
            return Err(CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: "No display".into(),
            });
        }
        Ok(Self::scene())
    }

    async fn analyze_scene_from_image(
        &self,
        _: Vec<u8>,
        _: String,
        a: Option<&str>,
        s: Option<&str>,
    ) -> Result<UiScene, CoreError> {
        self.analyze_scene(a, s).await
    }

    fn name(&self) -> &str {
        "e2e"
    }
}

struct E2eProbe {
    valid: Mutex<bool>,
}

impl E2eProbe {
    fn new() -> Self {
        Self {
            valid: Mutex::new(true),
        }
    }
    fn set_valid(&self, v: bool) {
        *self.valid.lock().unwrap() = v;
    }
}

#[async_trait]
impl FocusProbe for E2eProbe {
    async fn current_focus(&self) -> Result<FocusSnapshot, CoreError> {
        Ok(FocusSnapshot {
            app_name: "TestApp".into(),
            window_title: "Window".into(),
            pid: 1234,
            bounds: None,
            captured_at: Utc::now(),
            focus_hash: "e2e-hash".into(),
        })
    }

    async fn validate_execution_binding(
        &self,
        _: &ExecutionBinding,
    ) -> Result<FocusValidation, CoreError> {
        let v = *self.valid.lock().unwrap();
        Ok(FocusValidation {
            valid: v,
            reason: if v { None } else { Some("Drifted".into()) },
            current_focus: None,
        })
    }
}

struct E2eOverlay {
    show: AtomicUsize,
    clear: AtomicUsize,
}

impl E2eOverlay {
    fn new() -> Self {
        Self {
            show: AtomicUsize::new(0),
            clear: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl OverlayDriver for E2eOverlay {
    async fn show_highlights(&self, _: HighlightRequest) -> Result<HighlightHandle, CoreError> {
        self.show.fetch_add(1, Ordering::Relaxed);
        Ok(HighlightHandle {
            handle_id: uuid::Uuid::new_v4().to_string(),
            rendered_at: Utc::now(),
            target_count: 1,
        })
    }
    async fn clear_highlights(&self, _: &str) -> Result<(), CoreError> {
        self.clear.fetch_add(1, Ordering::Relaxed);
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

// ── Harness ─────────────────────────────────────────────────────────

struct Harness {
    svc: Arc<GuiInteractionService>,
    finder: Arc<E2eFinder>,
    probe: Arc<E2eProbe>,
    overlay: Arc<E2eOverlay>,
}

fn harness() -> Harness {
    let f = Arc::new(E2eFinder::new());
    let p = Arc::new(E2eProbe::new());
    let o = Arc::new(E2eOverlay::new());
    let s = Arc::new(GuiInteractionService::new(
        f.clone(),
        p.clone(),
        o.clone(),
        Some(TEST_HMAC_SECRET.into()),
    ));
    Harness {
        svc: s,
        finder: f,
        probe: p,
        overlay: o,
    }
}

fn req() -> GuiCreateSessionRequest {
    GuiCreateSessionRequest {
        app_name: Some("TestApp".into()),
        screen_id: None,
        min_confidence: None,
        max_candidates: None,
        session_ttl_secs: Some(300),
    }
}

async fn full_flow_to_confirm(h: &Harness) -> (String, String, GuiExecutionTicket) {
    let resp = h.svc.create_session(req()).await.unwrap();
    let (sid, tok) = (resp.session.session_id, resp.capability_token);

    h.svc
        .highlight_session(
            &sid,
            &tok,
            GuiHighlightRequest {
                candidate_ids: None,
            },
        )
        .await
        .unwrap();

    let session = h.svc.get_session(&sid, &tok).await.unwrap();
    let cid = session.candidates[0].element.element_id.clone();

    let ticket = h
        .svc
        .confirm_candidate(
            &sid,
            &tok,
            GuiConfirmRequest {
                candidate_id: cid,
                action: GuiActionRequest {
                    action_type: GuiActionType::Click,
                    text: None,
                },
                ticket_ttl_secs: Some(60),
            },
        )
        .await
        .unwrap();

    (sid, tok, ticket)
}

// ── Happy Path ──────────────────────────────────────────────────────

#[tokio::test]
async fn e2e_happy_path_full_flow() {
    let h = harness();
    let (sid, tok, ticket) = full_flow_to_confirm(&h).await;

    // Execute
    let _plan = h
        .svc
        .prepare_execution(&sid, &tok, GuiExecutionRequest { ticket })
        .await
        .unwrap();

    // Complete
    h.svc
        .complete_execution(&sid, true, Some("Done".into()), 1, 1)
        .await
        .unwrap();

    let final_s = h.svc.get_session(&sid, &tok).await.unwrap();
    assert_eq!(final_s.state, GuiSessionState::Executed);
    assert!(h.overlay.clear.load(Ordering::Relaxed) >= 1);
}

// ── Failure Scenarios ───────────────────────────────────────────────

#[tokio::test]
async fn e2e_permission_denied() {
    let h = harness();
    h.finder.fail_permission.store(true, Ordering::Relaxed);
    let err = h.svc.create_session(req()).await.unwrap_err();
    // Iter-91: PermissionDenied is a denial variant — maps to Forbidden with
    // wire code `gui.forbidden`. Pre-iter-91 this erroneously fell through to
    // Internal (`gui.internal_error`), losing the denial semantic for
    // frontend i18n. The new assertion validates the corrected mapping.
    assert!(
        matches!(err, GuiInteractionError::Forbidden { .. }),
        "expected Forbidden for permission denial; got: {err:?}"
    );
}

#[tokio::test]
async fn e2e_focus_drift_on_execute() {
    let h = harness();
    let (sid, tok, ticket) = full_flow_to_confirm(&h).await;
    h.probe.set_valid(false);
    let err = h
        .svc
        .prepare_execution(&sid, &tok, GuiExecutionRequest { ticket })
        .await
        .unwrap_err();
    assert!(
        matches!(err, GuiInteractionError::FocusDrift { .. }),
        "got: {err:?}"
    );
}

#[tokio::test]
async fn e2e_expired_ticket() {
    let h = harness();
    let (sid, tok, mut ticket) = full_flow_to_confirm(&h).await;
    ticket.expires_at = Utc::now() - chrono::Duration::seconds(100);
    let err = h
        .svc
        .prepare_execution(&sid, &tok, GuiExecutionRequest { ticket })
        .await
        .unwrap_err();
    assert!(
        matches!(err, GuiInteractionError::TicketInvalid { .. }),
        "got: {err:?}"
    );
}

// Session TTL boundary is tested at service level in M5 (m5_session_ttl_boundary_marks_expired)
// where expire_sessions() can be called directly. E2E cannot invoke the private cleanup method,
// and the 30s background cleanup interval makes real-time TTL testing impractical.

#[tokio::test]
async fn e2e_headless_no_display() {
    let h = harness();
    h.finder.should_fail.store(true, Ordering::Relaxed);
    let err = h.svc.create_session(req()).await.unwrap_err();
    assert!(
        matches!(err, GuiInteractionError::Internal { .. }),
        "got: {err:?}"
    );
}

#[tokio::test]
async fn e2e_missing_hmac_secret() {
    let f = Arc::new(E2eFinder::new());
    let p = Arc::new(E2eProbe::new());
    let o = Arc::new(E2eOverlay::new());
    let svc = GuiInteractionService::new(f, p, o, None);
    let err = svc.create_session(req()).await.unwrap_err();
    assert!(
        matches!(err, GuiInteractionError::Unavailable { .. }),
        "got: {err:?}"
    );
}
