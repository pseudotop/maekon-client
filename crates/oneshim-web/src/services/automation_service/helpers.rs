use chrono::{DateTime, Utc};

use oneshim_api_contracts::automation::{
    AutomationStatusDto, ExecuteSceneActionRequest, PoliciesDto, SceneActionType,
    SceneCalibrationDto,
};
use oneshim_core::config::{
    AiAccessMode, ExternalDataPolicy, LlmProviderType, OcrProviderType, PiiFilterLevel,
    SceneActionOverrideConfig, SceneIntelligenceConfig,
};
use oneshim_core::config_manager::ConfigManager;
use oneshim_core::models::audit::AuditStatus;
use oneshim_core::models::automation::AutomationAction;
use oneshim_core::models::intent::AutomationIntent;
use oneshim_core::models::ui_scene::UiScene;
use std::path::{Path as FsPath, PathBuf};

use crate::error::ApiError;
use crate::services::web_contexts::AutomationWebContext;

pub(crate) const AUTOMATION_SCENE_CALIBRATION_SCHEMA_VERSION: &str =
    crate::services::automation_service::AUTOMATION_SCENE_CALIBRATION_SCHEMA_VERSION;

pub(crate) fn require_config_manager(
    context: &AutomationWebContext,
) -> Result<&ConfigManager, ApiError> {
    context
        .config_manager
        .as_ref()
        .ok_or_else(|| ApiError::Internal("Config manager is not set".into()))
}

pub(crate) fn default_automation_status(pending: usize) -> AutomationStatusDto {
    AutomationStatusDto {
        enabled: false,
        sandbox_enabled: false,
        sandbox_profile: "Standard".to_string(),
        ocr_provider: "Local".to_string(),
        llm_provider: "Local".to_string(),
        ocr_source: "local".to_string(),
        llm_source: "local".to_string(),
        ocr_fallback_reason: None,
        llm_fallback_reason: None,
        external_data_policy: "PiiFilterStrict".to_string(),
        pending_audit_entries: pending,
    }
}

pub(crate) fn infer_runtime_source(
    access_mode: AiAccessMode,
    provider_is_remote: bool,
) -> &'static str {
    match access_mode {
        AiAccessMode::LocalModel => "local",
        AiAccessMode::ProviderSubscriptionCli => "cli-subscription",
        AiAccessMode::ProviderApiKey => infer_direct_runtime_source(provider_is_remote),
        AiAccessMode::ProviderOAuth => "oauth",
    }
}

fn infer_direct_runtime_source(provider_is_remote: bool) -> &'static str {
    if !provider_is_remote {
        return "local";
    }
    "remote"
}

pub(crate) fn resolve_ai_runtime_status(
    context: &AutomationWebContext,
    access_mode: AiAccessMode,
    ocr_provider: OcrProviderType,
    llm_provider: LlmProviderType,
) -> crate::AiRuntimeStatus {
    context
        .ai_runtime_status
        .clone()
        .unwrap_or_else(|| crate::AiRuntimeStatus {
            ocr_source: infer_runtime_source(
                access_mode,
                matches!(ocr_provider, OcrProviderType::Remote),
            )
            .to_string(),
            llm_source: infer_runtime_source(
                access_mode,
                matches!(llm_provider, LlmProviderType::Remote),
            )
            .to_string(),
            ocr_fallback_reason: None,
            llm_fallback_reason: None,
        })
}

pub(crate) fn default_policies() -> PoliciesDto {
    PoliciesDto {
        automation_enabled: false,
        sandbox_profile: "Standard".to_string(),
        sandbox_enabled: false,
        allow_network: false,
        external_data_policy: "PiiFilterStrict".to_string(),
        scene_action_override_enabled: false,
        scene_action_override_active: false,
        scene_action_override_reason: None,
        scene_action_override_approved_by: None,
        scene_action_override_expires_at: None,
        scene_action_override_issue: None,
    }
}

#[derive(Debug, Clone)]
pub(crate) struct SceneActionPolicyContext {
    pub(crate) policy: ExternalDataPolicy,
    pub(crate) pii_filter_level: PiiFilterLevel,
    pub(crate) override_enabled: bool,
    pub(crate) override_active: bool,
    pub(crate) override_reason: Option<String>,
    pub(crate) override_approved_by: Option<String>,
    pub(crate) override_expires_at: Option<DateTime<Utc>>,
    pub(crate) override_issue: Option<String>,
}

pub(crate) fn parse_audit_status(status_filter: &str) -> Result<AuditStatus, ApiError> {
    match status_filter {
        "Started" => Ok(AuditStatus::Started),
        "Completed" => Ok(AuditStatus::Completed),
        "Failed" => Ok(AuditStatus::Failed),
        "Denied" => Ok(AuditStatus::Denied),
        "Timeout" => Ok(AuditStatus::Timeout),
        _ => Err(ApiError::BadRequest(format!(
            "Invalid state filter: {}",
            status_filter
        ))),
    }
}

pub(crate) fn build_scene_action_intents(
    req: &ExecuteSceneActionRequest,
) -> Result<Vec<AutomationIntent>, ApiError> {
    if req.session_id.trim().is_empty() {
        return Err(ApiError::BadRequest("session_id is required".to_string()));
    }
    if req.element_id.trim().is_empty() {
        return Err(ApiError::BadRequest("element_id is required".to_string()));
    }
    if req.bbox_abs.width == 0 || req.bbox_abs.height == 0 {
        return Err(ApiError::BadRequest(
            "bbox_abs.width/height must be greater than 0.".to_string(),
        ));
    }

    let (center_x, center_y) = req.bbox_abs.center();

    match req.action_type {
        SceneActionType::Click => Ok(vec![AutomationIntent::Raw(AutomationAction::MouseClick {
            button: "left".to_string(),
            x: center_x,
            y: center_y,
        })]),
        SceneActionType::TypeText => {
            let text = req
                .text
                .as_ref()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
                .ok_or_else(|| {
                    ApiError::BadRequest("type_text action requires text".to_string())
                })?;

            Ok(vec![
                AutomationIntent::Raw(AutomationAction::MouseClick {
                    button: "left".to_string(),
                    x: center_x,
                    y: center_y,
                }),
                AutomationIntent::Raw(AutomationAction::KeyType { text }),
            ])
        }
    }
}

pub(crate) fn read_scene_intelligence_config(
    context: &AutomationWebContext,
) -> SceneIntelligenceConfig {
    context
        .config_manager
        .as_ref()
        .map(|config_manager| config_manager.get().ai_provider.scene_intelligence)
        .unwrap_or_default()
}

pub(crate) fn apply_scene_intelligence_filter(
    mut scene: UiScene,
    cfg: &SceneIntelligenceConfig,
) -> Result<UiScene, ApiError> {
    if !cfg.enabled {
        return Err(ApiError::BadRequest(
            "Scene intelligence is disabled.".to_string(),
        ));
    }

    scene.elements.retain(|element| {
        element.confidence.is_finite() && element.confidence >= cfg.min_confidence
    });

    scene.elements.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    scene.elements.truncate(cfg.max_elements);
    Ok(scene)
}

pub(crate) fn build_scene_calibration(
    scene: &UiScene,
    cfg: &SceneIntelligenceConfig,
) -> SceneCalibrationDto {
    let considered_elements = scene
        .elements
        .iter()
        .filter(|element| element.confidence.is_finite())
        .count();
    let sum_confidence: f64 = scene
        .elements
        .iter()
        .filter(|element| element.confidence.is_finite())
        .map(|element| element.confidence)
        .sum();
    let avg_confidence = if considered_elements == 0 {
        0.0
    } else {
        sum_confidence / considered_elements as f64
    };

    let mut reasons = Vec::new();
    if !cfg.calibration_enabled {
        reasons.push("calibration disabled by configuration".to_string());
    } else {
        if considered_elements < cfg.calibration_min_elements {
            reasons.push(format!(
                "insufficient elements: {} < {}",
                considered_elements, cfg.calibration_min_elements
            ));
        }
        if avg_confidence < cfg.calibration_min_avg_confidence {
            reasons.push(format!(
                "low average confidence: {:.3} < {:.3}",
                avg_confidence, cfg.calibration_min_avg_confidence
            ));
        }
    }

    let passed = cfg.calibration_enabled && reasons.is_empty();

    SceneCalibrationDto {
        schema_version: AUTOMATION_SCENE_CALIBRATION_SCHEMA_VERSION.to_string(),
        scene_id: scene.scene_id.clone(),
        total_elements: scene.elements.len(),
        considered_elements,
        avg_confidence,
        min_confidence: cfg.min_confidence,
        min_required_elements: cfg.calibration_min_elements,
        min_required_avg_confidence: cfg.calibration_min_avg_confidence,
        passed,
        reasons,
    }
}

pub(crate) fn evaluate_scene_action_override(
    cfg: &SceneActionOverrideConfig,
    now: DateTime<Utc>,
) -> (bool, Option<String>) {
    if !cfg.enabled {
        return (false, None);
    }

    let reason = cfg.reason.as_deref().map(str::trim).unwrap_or_default();
    if reason.is_empty() {
        return (
            false,
            Some("Override is invalid because reason is empty.".to_string()),
        );
    }

    let approved_by = cfg
        .approved_by
        .as_deref()
        .map(str::trim)
        .unwrap_or_default();
    if approved_by.is_empty() {
        return (
            false,
            Some("Override is invalid because approved_by is empty.".to_string()),
        );
    }

    let Some(expires_at) = cfg.expires_at else {
        return (
            false,
            Some("Override is invalid because expires_at is missing.".to_string()),
        );
    };

    if expires_at <= now {
        return (false, Some("Override TTL has expired.".to_string()));
    }

    (true, None)
}

pub(crate) fn read_scene_action_policy(context: &AutomationWebContext) -> SceneActionPolicyContext {
    if let Some(config_manager) = context.config_manager.as_ref() {
        let config = config_manager.get();
        let override_cfg = &config.ai_provider.scene_action_override;
        let (override_active, override_issue) =
            evaluate_scene_action_override(override_cfg, Utc::now());

        SceneActionPolicyContext {
            policy: config.ai_provider.external_data_policy,
            pii_filter_level: config.privacy.pii_filter_level,
            override_enabled: override_cfg.enabled,
            override_active,
            override_reason: override_cfg.reason.clone(),
            override_approved_by: override_cfg.approved_by.clone(),
            override_expires_at: override_cfg.expires_at,
            override_issue,
        }
    } else {
        SceneActionPolicyContext {
            policy: ExternalDataPolicy::PiiFilterStrict,
            pii_filter_level: PiiFilterLevel::Standard,
            override_enabled: false,
            override_active: false,
            override_reason: None,
            override_approved_by: None,
            override_expires_at: None,
            override_issue: None,
        }
    }
}

pub(crate) fn enforce_scene_action_privacy(
    context: &AutomationWebContext,
    req: &ExecuteSceneActionRequest,
) -> Result<SceneActionPolicyContext, ApiError> {
    let policy_context = read_scene_action_policy(context);
    let allow_sensitive = req.allow_sensitive_input.unwrap_or(false);
    let override_active = policy_context.override_active;
    let override_hint = policy_context
        .override_issue
        .as_ref()
        .map(|issue| format!(" current override state: {issue}"))
        .unwrap_or_default();

    match (policy_context.policy, req.action_type) {
        (ExternalDataPolicy::PiiFilterStrict, SceneActionType::TypeText)
            if !allow_sensitive && !override_active =>
        {
            return Err(ApiError::BadRequest(format!(
                "PiiFilterStrict blocks type_text actions. Pass allow_sensitive_input=true or configure a valid override.{override_hint}"
            )));
        }
        (ExternalDataPolicy::PiiFilterStandard, SceneActionType::TypeText)
            if !allow_sensitive && !override_active =>
        {
            return Err(ApiError::BadRequest(format!(
                "PiiFilterStandard requires allow_sensitive_input=true or a valid override for type_text actions.{override_hint}"
            )));
        }
        _ => {}
    }

    Ok(policy_context)
}

pub(crate) fn infer_image_format(path: &FsPath) -> String {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .filter(|ext| !ext.is_empty())
        .unwrap_or_else(|| "webp".to_string())
}

pub(crate) fn candidate_frame_paths(base: &FsPath, raw_relative: &FsPath) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    let base_name = base
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let starts_with_frames = raw_relative
        .components()
        .next()
        .map(|c| c.as_os_str() == "frames")
        .unwrap_or(false);

    candidates.push(base.join(raw_relative));

    if base_name == "frames" && starts_with_frames {
        if let Some(parent) = base.parent() {
            candidates.push(parent.join(raw_relative));
        }
    } else if base_name != "frames" && !starts_with_frames {
        candidates.push(base.join("frames").join(raw_relative));
    }

    candidates
}

pub(crate) fn resolve_frame_image_path(
    context: &AutomationWebContext,
    stored_path: &str,
) -> Result<PathBuf, ApiError> {
    let path = PathBuf::from(stored_path);
    if path.is_absolute() {
        return Ok(path);
    }

    let Some(base) = context.frames_dir.as_ref() else {
        return Err(ApiError::Internal(
            "Frame path root is not configured; frame_id queries cannot be resolved.".to_string(),
        ));
    };

    let candidates = candidate_frame_paths(base, &path);
    for candidate in &candidates {
        if candidate.exists() {
            return Ok(candidate.clone());
        }
    }

    Ok(candidates
        .into_iter()
        .next()
        .unwrap_or_else(|| base.join(path)))
}
