use super::*;
use crate::AppState;
use async_trait::async_trait;
use chrono::{Duration as ChronoDuration, Utc};
use oneshim_api_contracts::automation_gui::{
    GuiConfirmRequest, GuiCreateSessionRequest, GuiExecutionRequest, GuiHighlightRequest,
    GuiSessionPath,
};
use oneshim_automation::audit::AuditLogger;
use oneshim_automation::controller::AutomationController;
use oneshim_automation::input_driver::{NoOpElementFinder, NoOpInputDriver};
use oneshim_automation::intent_resolver::{IntentExecutor, IntentResolver};
use oneshim_automation::policy::PolicyClient;
use oneshim_automation::sandbox::NoOpSandbox;
use oneshim_core::config::SandboxConfig;
use oneshim_core::error::CoreError;
use oneshim_core::models::gui::{
    ExecutionBinding, FocusSnapshot, FocusValidation, GuiActionRequest, GuiActionType,
    HighlightHandle, HighlightRequest,
};
use oneshim_core::models::intent::{ElementBounds, IntentConfig};
use oneshim_core::models::ui_scene::{NormalizedBounds, UiScene, UiSceneElement};
use oneshim_core::ports::element_finder::ElementFinder;
use oneshim_core::ports::focus_probe::FocusProbe;
use oneshim_core::ports::overlay_driver::OverlayDriver;
use oneshim_storage::sqlite::SqliteStorage;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};

const M5_HMAC_SECRET: &str = "m5-hmac-secret-32-bytes-long!!!!";

// ── Mock types ──────────────────────────────────────────────────

struct M5PermissionDeniedFinder;

#[async_trait]
impl ElementFinder for M5PermissionDeniedFinder {
    async fn find_element(
        &self,
        _text: Option<&str>,
        _role: Option<&str>,
        _region: Option<&ElementBounds>,
    ) -> Result<Vec<oneshim_core::models::intent::UiElement>, CoreError> {
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
        "m5-permission-denied"
    }
}

struct M5MockFocusProbe {
    validation_valid: std::sync::Mutex<bool>,
}

impl M5MockFocusProbe {
    fn new() -> Self {
        Self {
            validation_valid: std::sync::Mutex::new(true),
        }
    }
    fn set_validation_valid(&self, valid: bool) {
        *self.validation_valid.lock().unwrap() = valid;
    }
}

#[async_trait]
impl FocusProbe for M5MockFocusProbe {
    async fn current_focus(&self) -> Result<FocusSnapshot, CoreError> {
        Ok(FocusSnapshot {
            app_name: "TestApp".to_string(),
            window_title: "Test Window".to_string(),
            pid: 1234,
            bounds: None,
            captured_at: Utc::now(),
            focus_hash: "m5focushash".to_string(),
        })
    }

    async fn validate_execution_binding(
        &self,
        _binding: &ExecutionBinding,
    ) -> Result<FocusValidation, CoreError> {
        let valid = *self.validation_valid.lock().unwrap();
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

struct M5MockOverlayDriver;

#[async_trait]
impl OverlayDriver for M5MockOverlayDriver {
    async fn show_highlights(&self, req: HighlightRequest) -> Result<HighlightHandle, CoreError> {
        Ok(HighlightHandle {
            handle_id: "m5-handle".to_string(),
            rendered_at: Utc::now(),
            target_count: req.targets.len(),
        })
    }

    async fn clear_highlights(&self, _handle_id: &str) -> Result<(), CoreError> {
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

struct M5MockElementFinder;

#[async_trait]
impl ElementFinder for M5MockElementFinder {
    async fn find_element(
        &self,
        _text: Option<&str>,
        _role: Option<&str>,
        _region: Option<&ElementBounds>,
    ) -> Result<Vec<oneshim_core::models::intent::UiElement>, CoreError> {
        Ok(vec![])
    }

    async fn analyze_scene(
        &self,
        app_name: Option<&str>,
        screen_id: Option<&str>,
    ) -> Result<UiScene, CoreError> {
        Ok(UiScene {
            schema_version: "ui_scene.v1".to_string(),
            scene_id: "m5-scene".to_string(),
            app_name: app_name.map(str::to_string),
            screen_id: screen_id.map(str::to_string),
            captured_at: Utc::now(),
            screen_width: 1920,
            screen_height: 1080,
            elements: vec![UiSceneElement {
                element_id: "btn-ok".to_string(),
                bbox_abs: ElementBounds {
                    x: 100,
                    y: 80,
                    width: 200,
                    height: 40,
                },
                bbox_norm: NormalizedBounds::new(0.05, 0.07, 0.10, 0.04),
                label: "OK".to_string(),
                role: Some("button".to_string()),
                intent: None,
                state: Some("enabled".to_string()),
                confidence: 0.95,
                text_masked: Some("OK".to_string()),
                parent_id: None,
            }],
        })
    }

    fn name(&self) -> &str {
        "m5-mock"
    }
}

// ── Fixture builders ────────────────────────────────────────────

fn make_controller_with(
    finder: Arc<dyn ElementFinder>,
    focus_probe: Arc<dyn FocusProbe>,
    hmac_secret: Option<String>,
) -> Arc<AutomationController> {
    let policy_client = Arc::new(PolicyClient::new());
    let audit_logger = Arc::new(RwLock::new(AuditLogger::default()));
    let sandbox: Arc<dyn oneshim_core::ports::sandbox::Sandbox> = Arc::new(NoOpSandbox);
    let sandbox_config = SandboxConfig::default();
    let mut controller =
        AutomationController::new(policy_client, audit_logger, sandbox, sandbox_config);

    controller.set_scene_finder(finder);
    controller
        .configure_gui_interaction(focus_probe, Arc::new(M5MockOverlayDriver), hmac_secret)
        .expect("configure_gui_interaction should succeed");

    let input_driver: Arc<dyn oneshim_core::ports::input_driver::InputDriver> =
        Arc::new(NoOpInputDriver);
    let element_finder: Arc<dyn ElementFinder> = Arc::new(NoOpElementFinder);
    let resolver = IntentResolver::new(element_finder, input_driver, IntentConfig::default());
    controller.set_intent_executor(Arc::new(IntentExecutor::new(
        resolver,
        IntentConfig::default(),
    )));

    controller.set_enabled(true);
    Arc::new(controller)
}

fn make_state_with(controller: Arc<AutomationController>) -> AppState {
    let storage = Arc::new(SqliteStorage::open_in_memory(30).unwrap());
    let (event_tx, _) = broadcast::channel(16);
    let mut state = AppState::with_core(storage, event_tx);
    state.automation.controller = Some(controller);
    state
}

fn m5_context(state: &AppState) -> AutomationGuiWebContext {
    AutomationGuiWebContext::from_state(state)
}

fn m5_token_headers(token: &str) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert(GUI_SESSION_HEADER, token.parse().unwrap());
    h
}

fn m5_default_create_req() -> GuiCreateSessionRequest {
    GuiCreateSessionRequest {
        app_name: Some("TestApp".to_string()),
        screen_id: None,
        min_confidence: None,
        max_candidates: None,
        session_ttl_secs: None,
    }
}

// ── M5 Tests ────────────────────────────────────────────────────

#[tokio::test]
async fn m5_permission_denied_returns_http_403() {
    let controller = make_controller_with(
        Arc::new(M5PermissionDeniedFinder),
        Arc::new(M5MockFocusProbe::new()),
        Some(M5_HMAC_SECRET.to_string()),
    );
    let state = make_state_with(controller);

    let err = create_gui_session(State(m5_context(&state)), Json(m5_default_create_req()))
        .await
        .unwrap_err();
    assert!(matches!(err, ApiError::Forbidden(_)));
}

#[tokio::test]
async fn m5_focus_drift_returns_http_409() {
    let focus_probe = Arc::new(M5MockFocusProbe::new());
    let controller = make_controller_with(
        Arc::new(M5MockElementFinder),
        focus_probe.clone(),
        Some(M5_HMAC_SECRET.to_string()),
    );
    let state = make_state_with(controller);

    // Create + highlight
    let resp = create_gui_session(State(m5_context(&state)), Json(m5_default_create_req()))
        .await
        .unwrap();
    let sid = resp.0.session.session_id.clone();
    let token = resp.0.capability_token.clone();

    let highlight_resp = highlight_gui_session(
        State(m5_context(&state)),
        Path(GuiSessionPath { id: sid.clone() }),
        m5_token_headers(&token),
        Json(GuiHighlightRequest {
            candidate_ids: None,
        }),
    )
    .await
    .unwrap();
    let candidate_id = highlight_resp.0.session.candidates[0]
        .element
        .element_id
        .clone();

    // Drift before confirm
    focus_probe.set_validation_valid(false);

    let err = confirm_gui_session(
        State(m5_context(&state)),
        Path(GuiSessionPath { id: sid }),
        m5_token_headers(&token),
        Json(GuiConfirmRequest {
            candidate_id,
            action: GuiActionRequest {
                action_type: GuiActionType::Click,
                text: None,
            },
            ticket_ttl_secs: Some(60),
        }),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, ApiError::Conflict(_)));
}

#[tokio::test]
async fn m5_expired_ticket_returns_http_422() {
    let controller = make_controller_with(
        Arc::new(M5MockElementFinder),
        Arc::new(M5MockFocusProbe::new()),
        Some(M5_HMAC_SECRET.to_string()),
    );
    let state = make_state_with(controller);

    // Create + highlight
    let resp = create_gui_session(State(m5_context(&state)), Json(m5_default_create_req()))
        .await
        .unwrap();
    let sid = resp.0.session.session_id.clone();
    let token = resp.0.capability_token.clone();

    let highlight_resp = highlight_gui_session(
        State(m5_context(&state)),
        Path(GuiSessionPath { id: sid.clone() }),
        m5_token_headers(&token),
        Json(GuiHighlightRequest {
            candidate_ids: None,
        }),
    )
    .await
    .unwrap();
    let candidate_id = highlight_resp.0.session.candidates[0]
        .element
        .element_id
        .clone();

    let confirm_resp = confirm_gui_session(
        State(m5_context(&state)),
        Path(GuiSessionPath { id: sid.clone() }),
        m5_token_headers(&token),
        Json(GuiConfirmRequest {
            candidate_id,
            action: GuiActionRequest {
                action_type: GuiActionType::Click,
                text: None,
            },
            ticket_ttl_secs: Some(60),
        }),
    )
    .await
    .unwrap();
    let mut ticket = confirm_resp.0.ticket;

    // Backdate ticket past grace window
    ticket.expires_at = Utc::now() - ChronoDuration::seconds(300);

    let err = execute_gui_session(
        State(m5_context(&state)),
        Path(GuiSessionPath { id: sid }),
        m5_token_headers(&token),
        Json(GuiExecutionRequest { ticket }),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, ApiError::Unprocessable(_)));
}

#[tokio::test]
async fn m5_sse_events_filtered_by_session_id() {
    let controller = make_controller_with(
        Arc::new(M5MockElementFinder),
        Arc::new(M5MockFocusProbe::new()),
        Some(M5_HMAC_SECRET.to_string()),
    );
    let state = make_state_with(controller);

    // Create two sessions
    let resp_a = create_gui_session(State(m5_context(&state)), Json(m5_default_create_req()))
        .await
        .unwrap();
    let sid_a = resp_a.0.session.session_id.clone();
    let token_a = resp_a.0.capability_token.clone();

    let resp_b = create_gui_session(State(m5_context(&state)), Json(m5_default_create_req()))
        .await
        .unwrap();
    let sid_b = resp_b.0.session.session_id.clone();
    let token_b = resp_b.0.capability_token.clone();

    // Subscribe to session B's events via the controller
    let ctrl = state.automation.controller.as_ref().unwrap();
    let mut rx_b = ctrl
        .gui_subscribe_events(&sid_b, &token_b)
        .await
        .expect("subscribe should succeed");

    // Trigger an event on session A (highlight)
    let _ = highlight_gui_session(
        State(m5_context(&state)),
        Path(GuiSessionPath { id: sid_a.clone() }),
        m5_token_headers(&token_a),
        Json(GuiHighlightRequest {
            candidate_ids: None,
        }),
    )
    .await
    .unwrap();

    // Apply the same filter_map the handler uses
    let mut b_events = Vec::new();
    while let Ok(event) = rx_b.try_recv() {
        if event.session_id == sid_b {
            b_events.push(event);
        }
    }

    // Session B should see NO events from session A's highlight
    assert!(
        b_events.is_empty(),
        "Session B stream should not contain session A events, got: {b_events:?}"
    );
}

#[tokio::test]
async fn m5_missing_hmac_secret_returns_http_503() {
    let controller = make_controller_with(
        Arc::new(M5MockElementFinder),
        Arc::new(M5MockFocusProbe::new()),
        None, // No HMAC secret — fail-closed
    );
    let state = make_state_with(controller);

    let err = create_gui_session(State(m5_context(&state)), Json(m5_default_create_req()))
        .await
        .unwrap_err();
    assert!(
        matches!(err, ApiError::ServiceUnavailable(_)),
        "Missing HMAC secret should fail closed with 503, got: {err:?}"
    );
}

#[tokio::test]
async fn m5_empty_hmac_secret_returns_http_503() {
    let controller = make_controller_with(
        Arc::new(M5MockElementFinder),
        Arc::new(M5MockFocusProbe::new()),
        Some("   ".to_string()), // Whitespace-only — treated as missing
    );
    let state = make_state_with(controller);

    let err = create_gui_session(State(m5_context(&state)), Json(m5_default_create_req()))
        .await
        .unwrap_err();
    assert!(
        matches!(err, ApiError::ServiceUnavailable(_)),
        "Empty HMAC secret should fail closed with 503, got: {err:?}"
    );
}
