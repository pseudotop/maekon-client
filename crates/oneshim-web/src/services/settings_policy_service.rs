use oneshim_core::config::AppConfig;
use oneshim_core::ports::audit_log::AuditLogPort;
use std::sync::Arc;

pub(crate) fn emit_policy_change_events(
    audit_logger: Option<Arc<dyn AuditLogPort>>,
    previous: &AppConfig,
    next: &AppConfig,
) {
    if previous.ai_provider.allow_unredacted_external_ocr
        != next.ai_provider.allow_unredacted_external_ocr
    {
        log_policy_event(
            audit_logger.clone(),
            "policy.settings.allow_unredacted_external_ocr.changed",
            format!(
                "from={} to={}",
                previous.ai_provider.allow_unredacted_external_ocr,
                next.ai_provider.allow_unredacted_external_ocr
            ),
        );
    }

    let prev_override = &previous.ai_provider.scene_action_override;
    let next_override = &next.ai_provider.scene_action_override;
    let override_changed = prev_override.enabled != next_override.enabled
        || prev_override.reason != next_override.reason
        || prev_override.approved_by != next_override.approved_by
        || prev_override.expires_at != next_override.expires_at;

    if override_changed {
        log_policy_event(
            audit_logger.clone(),
            "policy.settings.scene_action_override.changed",
            format!(
                "from_enabled={} to_enabled={} from_reason={:?} to_reason={:?} from_approved_by={:?} to_approved_by={:?} from_expires_at={:?} to_expires_at={:?}",
                prev_override.enabled,
                next_override.enabled,
                prev_override.reason.as_deref(),
                next_override.reason.as_deref(),
                prev_override.approved_by.as_deref(),
                next_override.approved_by.as_deref(),
                prev_override.expires_at.map(|value| value.to_rfc3339()),
                next_override.expires_at.map(|value| value.to_rfc3339()),
            ),
        );
    }

    let prev_scene = &previous.ai_provider.scene_intelligence;
    let next_scene = &next.ai_provider.scene_intelligence;
    let scene_changed = prev_scene.enabled != next_scene.enabled
        || prev_scene.overlay_enabled != next_scene.overlay_enabled
        || prev_scene.allow_action_execution != next_scene.allow_action_execution
        || (prev_scene.min_confidence - next_scene.min_confidence).abs() > f64::EPSILON
        || prev_scene.max_elements != next_scene.max_elements
        || prev_scene.calibration_enabled != next_scene.calibration_enabled
        || prev_scene.calibration_min_elements != next_scene.calibration_min_elements
        || (prev_scene.calibration_min_avg_confidence - next_scene.calibration_min_avg_confidence)
            .abs()
            > f64::EPSILON;

    if scene_changed {
        log_policy_event(
            audit_logger,
            "policy.settings.scene_intelligence.changed",
            format!(
                "enabled {}->{} overlay {}->{} allow_action_execution {}->{} min_confidence {:.2}->{:.2} max_elements {}->{} calibration_enabled {}->{} calibration_min_elements {}->{} calibration_min_avg_confidence {:.2}->{:.2}",
                prev_scene.enabled,
                next_scene.enabled,
                prev_scene.overlay_enabled,
                next_scene.overlay_enabled,
                prev_scene.allow_action_execution,
                next_scene.allow_action_execution,
                prev_scene.min_confidence,
                next_scene.min_confidence,
                prev_scene.max_elements,
                next_scene.max_elements,
                prev_scene.calibration_enabled,
                next_scene.calibration_enabled,
                prev_scene.calibration_min_elements,
                next_scene.calibration_min_elements,
                prev_scene.calibration_min_avg_confidence,
                next_scene.calibration_min_avg_confidence,
            ),
        );
    }
}

fn log_policy_event(
    audit_logger: Option<Arc<dyn AuditLogPort>>,
    action_type: &str,
    details: String,
) {
    let Some(logger) = audit_logger else {
        return;
    };
    let action_type = action_type.to_string();
    tokio::spawn(async move {
        logger.log_event(&action_type, "settings", &details).await;
    });
}
