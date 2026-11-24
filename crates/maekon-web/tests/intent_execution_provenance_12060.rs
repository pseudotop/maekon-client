//! Real router/controller boundary, using synthetic providers and a no-op driver.
#![cfg(test)]

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Request, StatusCode};
use maekon_automation::audit::{AuditLogAdapter, AuditLogger};
use maekon_automation::controller::AutomationController;
use maekon_automation::input_driver::{NoOpElementFinder, NoOpInputDriver};
use maekon_automation::intent_planner::LlmIntentPlanner;
use maekon_automation::intent_resolver::{IntentExecutor, IntentResolver};
use maekon_automation::policy::PolicyClient;
use maekon_automation::sandbox::NoOpSandbox;
use maekon_core::config::SandboxConfig;
use maekon_core::error::CoreError;
use maekon_core::models::intent::{AutomationIntent, IntentConfig, IntentResult};
use maekon_core::models::intent_execution::*;
use maekon_core::ports::llm_provider::{
    InterpretedAction, LlmCallHealth, LlmProvider, ScreenContext,
};
use maekon_storage::sqlite::SqliteStorage;
use maekon_web::{app_state::AppState, WebServer};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::{broadcast, RwLock};
use tower::ServiceExt;

const TOKEN: &str = "synthetic-local-profile-token";
const CANARY: &str = "CANARY_PROMPT CANARY_OCR WINDOW_CANARY https://secret.invalid /private/token";

struct Provider(Arc<AtomicUsize>);
#[async_trait]
impl LlmProvider for Provider {
    async fn interpret_intent(
        &self,
        _: &ScreenContext,
        hint: &str,
    ) -> Result<InterpretedAction, CoreError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        if hint.starts_with("failure") {
            return Err(CoreError::Network {
                code: maekon_core::error_codes::NetworkCode::Generic,
                message: CANARY.into(),
            });
        }
        if hint.starts_with("config-error") {
            return Err(CoreError::Config {
                code: maekon_core::error_codes::ConfigCode::Missing,
                message: CANARY.into(),
            });
        }
        if hint.starts_with("internal-error") {
            return Err(CoreError::Internal {
                code: maekon_core::error_codes::InternalCode::Generic,
                message: CANARY.into(),
            });
        }
        Ok(InterpretedAction {
            target_text: None,
            target_role: None,
            action_type: "hotkey".into(),
            confidence: if hint.starts_with("low") { 0.0 } else { 1.0 },
        })
    }
    fn intent_provider_class(&self) -> IntentProviderClass {
        IntentProviderClass::RemoteModel
    }
    fn provider_name(&self) -> &str {
        CANARY
    }
    fn is_external(&self) -> bool {
        false
    }
}

struct Harness {
    app: axum::Router,
    calls: Arc<AtomicUsize>,
    audit: Arc<RwLock<AuditLogger>>,
    health: Arc<LlmCallHealth>,
}

fn harness() -> Harness {
    harness_with_components(true, true)
}

fn harness_with_components(planner: bool, executor: bool) -> Harness {
    let audit = Arc::new(RwLock::new(AuditLogger::new(100, 10)));
    let calls = Arc::new(AtomicUsize::new(0));
    let mut controller = AutomationController::new(
        Arc::new(PolicyClient::new()),
        audit.clone(),
        Arc::new(NoOpSandbox),
        SandboxConfig::default(),
    );
    let finder = Arc::new(NoOpElementFinder);
    let driver = Arc::new(NoOpInputDriver);
    let config = IntentConfig {
        verify_after_action: false,
        max_retries: 0,
        ..Default::default()
    };
    controller.set_enabled(true);
    if planner {
        controller.set_intent_planner(Arc::new(LlmIntentPlanner::new(
            Arc::new(Provider(calls.clone())),
            finder.clone(),
        )));
    }
    if executor {
        controller.set_intent_executor(Arc::new(IntentExecutor::new(
            IntentResolver::new(finder, driver.clone(), config.clone()),
            config,
        )));
    }
    controller.set_inline_action_executor(driver);
    let (events, _) = broadcast::channel(16);
    let mut state =
        AppState::with_core(Arc::new(SqliteStorage::open_in_memory(30).unwrap()), events);
    state.auth.local_auth_token = Some(Arc::from(TOKEN));
    state.automation.controller = Some(Arc::new(controller));
    state.automation.audit_logger = Some(Arc::new(AuditLogAdapter::new(audit.clone())));
    let health = Arc::new(LlmCallHealth::default());
    state.automation.llm_call_health = Some(health.clone());
    let app = WebServer::build_router(state)
        .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))));
    Harness {
        app,
        calls,
        audit,
        health,
    }
}

async fn request(
    app: &axum::Router,
    method: &str,
    uri: &str,
    body: Value,
    token: Option<&str>,
) -> axum::response::Response {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(token) = token {
        request = request.header("x-local-auth", token);
    }
    app.clone()
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap()
}

async fn json_body(response: axum::response::Response) -> Value {
    serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap()
}

fn submitted(id: OpaqueIntentId, hint: &str) -> Value {
    json!({"command_id":"same-command", "session_id":"same-session", "intent_hint":hint, "submission_id":id})
}

async fn lookup(app: &axum::Router, id: OpaqueIntentId) -> Value {
    // Filters and pagination cannot turn exact lookup into "latest matching".
    let response = request(
        app,
        "GET",
        &format!("/api/automation/audit?submission_id={id}&limit=0&status=Denied"),
        Value::Null,
        Some(TOKEN),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    json_body(response).await
}

#[tokio::test]
async fn success_error_and_lost_response_recover_exact_execution_independent_of_health() {
    let h = harness();
    let a = OpaqueIntentId::new();
    let b = OpaqueIntentId::new();
    let first = request(
        &h.app,
        "POST",
        "/api/automation/execute-hint",
        submitted(a, "Ctrl+S"),
        Some(TOKEN),
    )
    .await;
    assert_eq!(first.status(), StatusCode::OK);
    let first = json_body(first).await;
    let a_receipt = first["execution_provenance"].clone();
    assert_eq!(a_receipt["action_outcome"], "completed");
    assert_eq!(a_receipt["planning_outcome"], "llm");
    h.health.record_failed();
    let second = request(
        &h.app,
        "POST",
        "/api/automation/execute-hint",
        submitted(b, &format!("failure {CANARY}")),
        Some(TOKEN),
    )
    .await;
    assert_eq!(second.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let second = json_body(second).await;
    assert_eq!(second["status"], 500);
    assert_eq!(
        second["execution_provenance"]["attempts"][0]["outcome"],
        "unavailable"
    );
    assert!(!second.to_string().contains("CANARY"));
    assert_ne!(
        second["execution_provenance"]["execution_id"],
        a_receipt["execution_id"]
    );
    assert_eq!(
        lookup(&h.app, a).await[0]["execution_provenance"],
        a_receipt
    );
    assert_eq!(
        lookup(&h.app, b).await[0]["execution_provenance"],
        second["execution_provenance"]
    );
    assert_eq!(lookup(&h.app, b).await[0]["status"], "Failed");
    h.health.record_ok();
    assert_eq!(
        lookup(&h.app, a).await[0]["execution_provenance"],
        a_receipt
    );

    let lost = OpaqueIntentId::new();
    drop(
        request(
            &h.app,
            "POST",
            "/api/automation/execute-hint",
            submitted(lost, "Ctrl+S"),
            Some(TOKEN),
        )
        .await,
    );
    let recovered = lookup(&h.app, lost).await;
    assert_eq!(
        recovered[0]["execution_provenance"]["submission_id"],
        lost.to_string()
    );
    assert_eq!(
        recovered[0]["execution_provenance"]["action_outcome"],
        "completed"
    );
    assert_eq!(h.calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn correlation_is_not_authority_or_permission_to_replay_or_select_duplicate() {
    let h = harness();
    let id = OpaqueIntentId::new();
    let body = submitted(id, "Ctrl+S");
    for token in [None, Some("wrong-profile-token")] {
        let response = request(
            &h.app,
            "POST",
            "/api/automation/execute-hint",
            body.clone(),
            token,
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    assert_eq!(h.calls.load(Ordering::SeqCst), 0);
    let first = json_body(
        request(
            &h.app,
            "POST",
            "/api/automation/execute-hint",
            body.clone(),
            Some(TOKEN),
        )
        .await,
    )
    .await;
    let response = request(
        &h.app,
        "GET",
        &format!("/api/automation/audit?submission_id={id}"),
        Value::Null,
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let other_profile = harness();
    assert_eq!(lookup(&other_profile.app, id).await, json!([]));
    let duplicate = json_body(
        request(
            &h.app,
            "POST",
            "/api/automation/execute-hint",
            body,
            Some(TOKEN),
        )
        .await,
    )
    .await;
    assert_ne!(
        first["execution_provenance"]["execution_id"],
        duplicate["execution_provenance"]["execution_id"]
    );
    assert_eq!(lookup(&h.app, id).await, json!([]));
    assert_eq!(lookup(&h.app, OpaqueIntentId::new()).await, json!([]));
    assert_eq!(h.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn planner_rejection_keeps_http_400_and_error_receipt_without_private_text() {
    let h = harness();
    let id = OpaqueIntentId::new();
    let response = request(
        &h.app,
        "POST",
        "/api/automation/execute-hint",
        submitted(id, &format!("low Ctrl+S {CANARY}")),
        Some(TOKEN),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = json_body(response).await;
    assert_eq!(body["code"], "validation.invalid_arguments");
    assert_eq!(
        body["execution_provenance"]["action_outcome"],
        "not_started"
    );
    assert_eq!(
        body["execution_provenance"]["observation_reason"],
        "confidence_rejected"
    );
    assert_eq!(
        body["execution_provenance"]["attempts"][0]["outcome"],
        "succeeded"
    );
    assert!(!body.to_string().contains("CANARY"));
    assert_eq!(
        lookup(&h.app, id).await[0]["execution_provenance"],
        body["execution_provenance"]
    );
}

#[tokio::test]
async fn missing_planner_and_executor_keep_http_meaning_and_exact_observed_history() {
    for (planner, executor, provider_calls) in [(false, true, 0), (true, false, 1)] {
        let h = harness_with_components(planner, executor);
        let id = OpaqueIntentId::new();
        let response = request(
            &h.app,
            "POST",
            "/api/automation/execute-hint",
            submitted(id, "Ctrl+S"),
            Some(TOKEN),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = json_body(response).await;
        assert_eq!(body["code"], "validation.invalid_arguments");
        let receipt = &body["execution_provenance"];
        assert_eq!(receipt["submission_id"], id.to_string());
        assert_eq!(receipt["action_outcome"], "not_started");
        assert_eq!(
            receipt["attempts"].as_array().unwrap().len(),
            provider_calls
        );
        assert_eq!(
            lookup(&h.app, id).await[0]["execution_provenance"],
            *receipt
        );
        assert_eq!(h.calls.load(Ordering::SeqCst), provider_calls);
        assert!(!body.to_string().contains("CANARY"));
    }
}

#[tokio::test]
async fn explicit_retry_has_a_new_identity_and_only_links_a_known_execution() {
    let h = harness();
    let a = OpaqueIntentId::new();
    let first_response = request(
        &h.app,
        "POST",
        "/api/automation/execute-hint",
        submitted(a, "Ctrl+S"),
        Some(TOKEN),
    )
    .await;
    assert_eq!(first_response.status(), StatusCode::OK);
    let first = json_body(first_response).await["execution_provenance"].clone();
    let b = OpaqueIntentId::new();
    let mut payload = submitted(b, "Ctrl+S");
    payload["retry_of"] = first["execution_id"].clone();
    let response = request(
        &h.app,
        "POST",
        "/api/automation/execute-hint",
        payload,
        Some(TOKEN),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let second = json_body(response).await["execution_provenance"].clone();
    assert_eq!(second["retry_of"], first["execution_id"]);
    assert_eq!(second["submission_id"], b.to_string());
    assert_ne!(second["execution_id"], first["execution_id"]);
    assert_eq!(lookup(&h.app, a).await[0]["execution_provenance"], first);
    assert_eq!(lookup(&h.app, b).await[0]["execution_provenance"], second);

    let c = OpaqueIntentId::new();
    let mut payload = submitted(c, "Ctrl+S");
    payload["retry_of"] = json!(OpaqueIntentId::new());
    let response = request(
        &h.app,
        "POST",
        "/api/automation/execute-hint",
        payload,
        Some(TOKEN),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let third = json_body(response).await;
    assert_eq!(third["execution_provenance"]["retry_of"], Value::Null);
    assert_eq!(
        third["execution_provenance"]["submission_id"],
        c.to_string()
    );
    assert_eq!(h.calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn provider_config_and_internal_failures_remain_http_500_with_safe_receipts() {
    for (hint, reason) in [
        ("config-error", "unavailable"),
        ("internal-error", "provider_error"),
    ] {
        let h = harness();
        let id = OpaqueIntentId::new();
        let response = request(
            &h.app,
            "POST",
            "/api/automation/execute-hint",
            submitted(id, hint),
            Some(TOKEN),
        )
        .await;
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = json_body(response).await;
        assert_eq!(body["code"], "internal.generic");
        assert_eq!(
            body["execution_provenance"]["attempts"][0]["reason"],
            reason
        );
        assert_eq!(
            body["execution_provenance"]["action_outcome"],
            "not_started"
        );
        assert_eq!(
            body["execution_provenance"]["submission_id"],
            id.to_string()
        );
        assert_eq!(
            lookup(&h.app, id).await[0]["execution_provenance"],
            body["execution_provenance"]
        );
        assert_eq!(h.calls.load(Ordering::SeqCst), 1);
        assert!(!body.to_string().contains("CANARY"));
    }
}

#[derive(Deserialize)]
struct LegacyResponse {
    command_id: String,
    session_id: String,
    planned_intent: AutomationIntent,
    result: IntentResult,
}

#[tokio::test]
async fn old_request_and_response_consumers_keep_working_and_invalid_correlation_is_rejected() {
    let h = harness();
    let response = request(
        &h.app,
        "POST",
        "/api/automation/execute-hint",
        json!({"session_id":"old-session","intent_hint":"Ctrl+S"}),
        Some(TOKEN),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["execution_provenance"]["submission_id"], Value::Null);
    let legacy: LegacyResponse = serde_json::from_value(body).unwrap();
    assert!(!legacy.command_id.is_empty());
    assert_eq!(legacy.session_id, "old-session");
    assert!(legacy.result.success);
    assert!(matches!(
        legacy.planned_intent,
        AutomationIntent::ExecuteHotkey { .. }
    ));
    let response = request(
        &h.app,
        "POST",
        "/api/automation/execute-hint",
        json!({"session_id":"s","intent_hint":"Ctrl+S","submission_id":CANARY}),
        Some(TOKEN),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(h.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn audit_serializer_preserves_safe_receipts_and_rejects_forbidden_field_insertion() {
    let h = harness();
    let body = json_body(
        request(
            &h.app,
            "POST",
            "/api/automation/execute-hint",
            submitted(OpaqueIntentId::new(), "Ctrl+S"),
            Some(TOKEN),
        )
        .await,
    )
    .await;
    let rows = json_body(
        request(
            &h.app,
            "GET",
            "/api/automation/audit",
            Value::Null,
            Some(TOKEN),
        )
        .await,
    )
    .await;
    let recorded = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["action_type"] == "intent.execution.observation")
        .unwrap();
    assert_eq!(
        recorded["execution_provenance"]["execution_id"],
        body["execution_provenance"]["execution_id"]
    );
    // Queue acceptance is not a database commit; the persisted snapshot was
    // captured before finalization and cannot claim durable completeness.
    assert_eq!(recorded["execution_provenance"]["observation"], "partial");
    assert_eq!(
        recorded["execution_provenance"]["storage"],
        "process_memory"
    );
    assert!(recorded.get("details").is_none());
    let mut injected = body["execution_provenance"].clone();
    injected["raw_prompt"] = json!(CANARY);
    h.audit.write().await.log_event(
        "intent.execution.observation",
        "test-session",
        &injected.to_string(),
    );
    let rows = json_body(
        request(
            &h.app,
            "GET",
            "/api/automation/audit",
            Value::Null,
            Some(TOKEN),
        )
        .await,
    )
    .await;
    assert!(!rows.to_string().contains("CANARY"));
    assert!(!rows.to_string().contains("raw_prompt"));
    let observations: Vec<_> = rows
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["action_type"] == "intent.execution.observation")
        .collect();
    assert_eq!(observations.len(), 2);
    assert_eq!(
        observations
            .iter()
            .filter(|row| row.get("execution_provenance").is_some())
            .count(),
        1
    );
}
