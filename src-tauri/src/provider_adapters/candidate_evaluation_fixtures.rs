//! Synthetic ports for #12423. No OS, credential discovery, CLI or model is reachable.

use super::*;
use async_trait::async_trait;
use chrono::Utc;
use maekon_automation::gui_interaction::{GuiCreateSessionRequest, GuiInteractionService};
use maekon_core::config::{AiAccessMode, ExternalDataPolicy, PiiFilterLevel, PrivacyConfig};
use maekon_core::consent::{ConsentManager, ConsentPermissions};
use maekon_core::error::CoreError;
use maekon_core::models::candidate_decision::{CandidateDecisionRequest, CandidateSnapshot};
use maekon_core::models::candidate_decision_policy::{
    CandidateDecisionPolicy, CandidateProvider, CandidateTrigger,
};
use maekon_core::models::context::{ProcessInfo, WindowInfo};
use maekon_core::models::event::ProcessDetail;
use maekon_core::models::gui::{
    ExecutionBinding, FocusSnapshot, FocusValidation, GuiCandidate, HighlightHandle,
    HighlightRequest,
};
use maekon_core::models::intent::{ElementBounds, UiElement};
use maekon_core::models::ui_scene::{NormalizedBounds, UiScene, UiSceneElement};
use maekon_core::ports::{
    element_finder::ElementFinder, focus_probe::FocusProbe, monitor::ProcessMonitor,
    overlay_driver::OverlayDriver, secret_store::SecretStore,
};
use maekon_storage::sqlite::SqliteStorage;
use serde::Deserialize;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

#[derive(Clone, Deserialize)]
pub(super) struct Candidate {
    pub id: String,
    pub label: String,
    pub recognition_confidence: f64,
}

#[derive(Clone, Deserialize)]
pub(super) struct Input {
    pub goal: String,
    pub candidates: Vec<Candidate>,
}

// Expected answers are deliberately absent from the executable's input type.
#[derive(Clone, Deserialize)]
pub(super) struct Sample {
    pub sample_id: String,
    pub family_id: String,
    pub input_sha256: String,
    pub permission_state: String,
    pub freshness: String,
    pub input: Input,
}

#[derive(Deserialize)]
pub(super) struct Corpus {
    pub samples: Vec<Sample>,
}

struct Scene(UiScene);

#[async_trait]
impl ElementFinder for Scene {
    async fn find_element(
        &self,
        _: Option<&str>,
        _: Option<&str>,
        _: Option<&ElementBounds>,
    ) -> Result<Vec<UiElement>, CoreError> {
        panic!("candidate collection must not target an OS element");
    }
    async fn analyze_scene(&self, _: Option<&str>, _: Option<&str>) -> Result<UiScene, CoreError> {
        Ok(self.0.clone())
    }
    fn name(&self) -> &str {
        "synthetic-gui-evaluation"
    }
}

struct Focus;

#[async_trait]
impl FocusProbe for Focus {
    async fn current_focus(&self) -> Result<FocusSnapshot, CoreError> {
        Ok(FocusSnapshot {
            app_name: "Fixture editor".into(),
            window_title: "Fixture".into(),
            pid: 1,
            bounds: None,
            captured_at: Utc::now(),
            focus_hash: "synthetic-focus".into(),
        })
    }
    async fn validate_execution_binding(
        &self,
        _: &ExecutionBinding,
    ) -> Result<FocusValidation, CoreError> {
        panic!("evaluation must never confirm or execute");
    }
}

struct NoOverlay;

#[async_trait]
impl OverlayDriver for NoOverlay {
    async fn show_highlights(&self, _: HighlightRequest) -> Result<HighlightHandle, CoreError> {
        panic!("no overlay")
    }
    async fn clear_highlights(&self, _: &str) -> Result<(), CoreError> {
        panic!("no overlay")
    }
    async fn show_detection(&self, _: &UiScene) -> Result<(), CoreError> {
        panic!("no overlay")
    }
    async fn clear_detection(&self) -> Result<(), CoreError> {
        panic!("no overlay")
    }
}

pub(super) async fn generate(input: &Input, limited: bool) -> Result<Vec<GuiCandidate>, String> {
    let scene = UiScene {
        schema_version: "ui_scene.v1".into(),
        scene_id: "synthetic-scene".into(),
        app_name: Some("Fixture editor".into()),
        screen_id: Some("synthetic-screen".into()),
        captured_at: Utc::now(),
        screen_width: 1000,
        screen_height: 1000,
        elements: input
            .candidates
            .iter()
            .map(|c| UiSceneElement {
                element_id: c.id.clone(),
                label: c.label.clone(),
                text_masked: Some(c.label.clone()),
                bbox_abs: ElementBounds {
                    x: 1,
                    y: 1,
                    width: 10,
                    height: 10,
                },
                bbox_norm: NormalizedBounds::new(0.001, 0.001, 0.01, 0.01),
                role: Some("button".into()),
                intent: None,
                state: Some("enabled".into()),
                confidence: c.recognition_confidence,
                parent_id: None,
            })
            .collect(),
    };
    let service = GuiInteractionService::new(
        Arc::new(Scene(scene)),
        Arc::new(Focus),
        Arc::new(NoOverlay),
        Some("synthetic-evaluation-hmac-not-a-user-secret".into()),
    );
    // This public production entrypoint owns cutoff, stable sorting and truncation.
    // The generated capability token stays inside this function and is never exported.
    service
        .create_session(GuiCreateSessionRequest {
            app_name: None,
            screen_id: None,
            min_confidence: limited.then_some(0.9),
            max_candidates: limited.then_some(2),
            session_ttl_secs: None,
        })
        .await
        .map(|created| created.session.candidates)
        .map_err(|error| error.code().to_owned())
}

struct Monitor(WindowInfo);

#[async_trait]
impl ProcessMonitor for Monitor {
    async fn get_active_window(&self) -> Result<Option<WindowInfo>, CoreError> {
        Ok(Some(self.0.clone()))
    }
    async fn get_top_processes(&self, _: usize) -> Result<Vec<ProcessInfo>, CoreError> {
        Ok(vec![])
    }
    async fn get_detailed_processes(
        &self,
        _: Option<u32>,
        _: usize,
    ) -> Result<Vec<ProcessDetail>, CoreError> {
        Ok(vec![])
    }
}

#[derive(Default)]
pub(super) struct NoSecrets(pub AtomicUsize);

#[async_trait]
impl SecretStore for NoSecrets {
    async fn retrieve(&self, _: &str, _: &str) -> Result<Option<String>, CoreError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        panic!("offline collection must not query credentials");
    }
    async fn store(&self, _: &str, _: &str, _: &str) -> Result<(), CoreError> {
        panic!("no credential writes")
    }
    async fn delete(&self, _: &str, _: &str) -> Result<(), CoreError> {
        panic!("no credential writes")
    }
    async fn delete_namespace(&self, _: &str) -> Result<(), CoreError> {
        panic!("no credential writes")
    }
}

pub(super) struct RuntimeFixture {
    pub request: CandidateDecisionRequest,
    pub rules: CandidateAssessmentRuntime,
    pub off: CandidateAssessmentRuntime,
    pub storage: Arc<SqliteStorage>,
    pub secrets: Arc<NoSecrets>,
    _dir: tempfile::TempDir,
}

pub(super) fn runtime(sample: &Sample, candidates: Vec<GuiCandidate>) -> RuntimeFixture {
    let window = WindowInfo {
        title: "Fixture".into(),
        app_name: "Fixture editor".into(),
        app_bundle_id: None,
        pid: 1,
        bounds: None,
    };
    let request = CandidateDecisionRequest::new(
        CandidateSnapshot {
            goal: sample.input.goal.clone(),
            scene_id: "synthetic-scene".into(),
            frame_id: "synthetic-frame".into(),
            generation: 1,
            window: window.clone(),
            candidates,
        },
        Instant::now(),
        Duration::from_secs(30),
    )
    .unwrap();
    let dir = tempfile::TempDir::new().unwrap();
    let consent = Arc::new(ConsentManager::new(dir.path().join("consent.json")));
    if sample.permission_state != "denied" {
        consent
            .grant_consent(
                ConsentPermissions {
                    full_text_extraction: true,
                    ocr_processing: true,
                    ..ConsentPermissions::default()
                },
                30,
            )
            .unwrap();
    }
    if sample.permission_state == "revoked" {
        consent.revoke_consent().unwrap();
    }
    let mut observed = window;
    if sample.freshness == "stale" {
        observed.title = "Changed fixture".into();
    }
    let privacy = ExternalOcrPrivacyGuard::new(
        consent,
        PiiFilterLevel::Strict,
        ExternalDataPolicy::PiiFilterStrict,
        PrivacyConfig::default(),
        Arc::new(Monitor(observed)),
        None,
    );
    let storage = Arc::new(SqliteStorage::open(&dir.path().join("audit.db"), 30, None).unwrap());
    let secrets = Arc::new(NoSecrets::default());
    let off = build_candidate_assessment_runtime(
        CandidateDecisionPolicy::default(),
        AiAccessMode::LocalModel,
        privacy.clone(),
        storage.clone(),
        None,
        Some(secrets.clone()),
    );
    let rules = build_candidate_assessment_runtime(
        CandidateDecisionPolicy {
            provider: Some(CandidateProvider::LocalRules),
            ..CandidateDecisionPolicy::default()
        },
        AiAccessMode::LocalModel,
        privacy,
        storage.clone(),
        None,
        Some(secrets.clone()),
    );
    let AssessmentRuntimeControl::Local(control) = &rules.control else {
        panic!("local rules must be constructible")
    };
    control.bind(&request).unwrap();
    control
        .approve(LocalAssessmentApproval {
            approval_reference: "synthetic-only-evaluator".into(),
            principal_namespace: "synthetic-principal".into(),
            policy_revision: "development-r1".into(),
            configuration_revision: "local-rules-only".into(),
            trigger: CandidateTrigger::UserRequest,
            max_attempts: 2,
            expires_at: Instant::now() + Duration::from_secs(60),
            model: None,
        })
        .unwrap();
    RuntimeFixture {
        request,
        rules,
        off,
        storage,
        secrets,
        _dir: dir,
    }
}
