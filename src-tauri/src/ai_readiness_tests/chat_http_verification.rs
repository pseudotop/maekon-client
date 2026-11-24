#![cfg(test)]
//! #12530: recorded HTTP Chat invocation evidence as a readiness input.

use super::*;
use maekon_core::ai_readiness::{AiCapabilityReadiness, AiReadinessAction};
use maekon_core::consent::ConsentPermissions;

const SURFACE: &str = "provider_surface.fixture.direct_http";

fn http_config(model: Option<&str>) -> AppConfig {
    let mut config = AppConfig::default_config();
    config.ai_provider.access_mode = AiAccessMode::ProviderApiKey;
    let mut llm_api = endpoint(SURFACE);
    llm_api.model = model.map(str::to_string);
    config.ai_provider.llm_api = Some(llm_api);
    config
}

fn full_text(granted: bool) -> ConsentPermissions {
    ConsentPermissions {
        full_text_extraction: granted,
        ..Default::default()
    }
}

fn readiness(
    config: &AppConfig,
    availability: FeatureAvailability,
    consent: &ConsentPermissions,
    verified: bool,
) -> AiReadinessSnapshot {
    build_ai_readiness_snapshot_with_local_preflight(
        &provider_snapshot(SURFACE, availability, None),
        config,
        config,
        consent,
        None,
        verified,
    )
}

fn chat_http(snapshot: &AiReadinessSnapshot) -> &AiCapabilityReadiness {
    snapshot
        .find(AiCapabilityId::ChatHttpApi)
        .expect("HTTP chat readiness")
}

#[test]
fn only_matching_evidence_makes_http_chat_ready() {
    let config = http_config(Some("model-a"));

    let unverified = readiness(
        &config,
        FeatureAvailability::Available,
        &full_text(true),
        false,
    );
    let chat = chat_http(&unverified);
    assert_eq!(chat.status, AiReadinessStatus::Blocked);
    assert_eq!(
        chat.reason_code,
        AiReadinessReasonCode::ProviderInvocationUnverified
    );
    assert_eq!(chat.action, AiReadinessAction::VerifyProviderInvocation);

    let verified = readiness(
        &config,
        FeatureAvailability::Available,
        &full_text(true),
        true,
    );
    let chat = chat_http(&verified);
    assert_eq!(chat.status, AiReadinessStatus::Ready);
    assert_eq!(chat.reason_code, AiReadinessReasonCode::Ready);
    assert_eq!(
        chat.dimensions.provider_invocation,
        AiProviderInvocationReadiness::Ready
    );
    assert_eq!(
        chat.dimensions.model_availability,
        AiModelAvailability::Available
    );
}

/// With no model configured the session uses the catalog default, and the
/// fingerprint names that model, so the evidence settles model availability.
#[test]
fn evidence_covers_the_default_model_an_empty_setting_resolves_to() {
    let config = http_config(None);

    let unverified = readiness(
        &config,
        FeatureAvailability::Available,
        &full_text(true),
        false,
    );
    assert_eq!(
        chat_http(&unverified).dimensions.model_availability,
        AiModelAvailability::Unavailable
    );

    let verified = readiness(
        &config,
        FeatureAvailability::Available,
        &full_text(true),
        true,
    );
    assert_eq!(chat_http(&verified).status, AiReadinessStatus::Ready);
}

#[test]
fn withdrawn_consent_blocks_http_chat_despite_evidence() {
    let config = http_config(Some("model-a"));
    let snapshot = readiness(
        &config,
        FeatureAvailability::Available,
        &full_text(false),
        true,
    );
    let chat = chat_http(&snapshot);

    assert_eq!(chat.status, AiReadinessStatus::Blocked);
    assert_eq!(chat.reason_code, AiReadinessReasonCode::ConsentRequired);
    assert_eq!(chat.action, AiReadinessAction::OpenPrivacyConsent);
    // The consent is the only blocker left: granting it alone is enough.
    let mut granted = chat.dimensions.clone();
    for consent in &mut granted.consent {
        consent.granted = true;
    }
    assert_eq!(
        evaluate_ai_readiness(AiCapabilityId::ChatHttpApi, granted).status,
        AiReadinessStatus::Ready
    );
}

#[test]
fn evidence_does_not_revive_a_surface_reported_unavailable() {
    let config = http_config(Some("model-a"));
    let snapshot = readiness(
        &config,
        FeatureAvailability::Unavailable,
        &full_text(true),
        true,
    );

    assert_eq!(
        chat_http(&snapshot).reason_code,
        AiReadinessReasonCode::ProviderInvocationUnavailable
    );
}

/// The analysis and summary routes share the HTTP endpoint axes, so a leak of
/// the Chat evidence into them would show up in their dimensions even where
/// an earlier blocker hides it from the reason code.
#[test]
fn evidence_changes_no_capability_other_than_http_chat() {
    let config = http_config(Some("model-a"));
    let consent = ConsentPermissions {
        full_text_extraction: true,
        ocr_processing: true,
        activity_pattern_learning: true,
        ..Default::default()
    };
    let unverified = readiness(&config, FeatureAvailability::Available, &consent, false);
    let verified = readiness(&config, FeatureAvailability::Available, &consent, true);

    assert_eq!(unverified.capabilities.len(), verified.capabilities.len());
    for (before, after) in unverified.capabilities.iter().zip(&verified.capabilities) {
        assert_eq!(before.capability_id, after.capability_id);
        if before.capability_id == AiCapabilityId::ChatHttpApi {
            assert_ne!(before, after);
        } else {
            assert_eq!(
                before, after,
                "{:?} must not read the HTTP Chat evidence",
                before.capability_id
            );
        }
    }
}

/// Readiness names the reason a restart is still needed before evidence can
/// matter: evidence recorded for the boot-time provider does not make a
/// saved, not yet applied provider change ready.
#[test]
fn evidence_does_not_skip_a_pending_restart() {
    let boot = http_config(Some("model-a"));
    let current = http_config(Some("model-b"));
    let snapshot = build_ai_readiness_snapshot_with_local_preflight(
        &provider_snapshot(SURFACE, FeatureAvailability::Available, None),
        &current,
        &boot,
        &full_text(true),
        None,
        true,
    );
    let chat = chat_http(&snapshot);

    assert_eq!(chat.status, AiReadinessStatus::Blocked);
    assert_eq!(chat.reason_code, AiReadinessReasonCode::RestartRequired);
}

/// #12561: an HTTP Chat session resolves an API-key binding only, so the
/// OAuth mode names the access-mode mismatch and points at the AI settings,
/// with or without recorded evidence. The same configuration in the API-key
/// mode is the positive control.
#[test]
fn provider_oauth_reports_the_mode_mismatch_despite_evidence() {
    let mut config = http_config(Some("model-a"));
    config.ai_provider.access_mode = AiAccessMode::ProviderOAuth;

    for verified in [false, true] {
        let snapshot = readiness(
            &config,
            FeatureAvailability::Available,
            &full_text(true),
            verified,
        );
        let chat = chat_http(&snapshot);
        assert_eq!(
            chat.status,
            AiReadinessStatus::Blocked,
            "verified={verified}"
        );
        assert_eq!(
            chat.reason_code,
            AiReadinessReasonCode::AccessModeMismatch,
            "verified={verified}"
        );
        assert_eq!(
            chat.action,
            AiReadinessAction::OpenAiSettings,
            "verified={verified}"
        );
    }

    config.ai_provider.access_mode = AiAccessMode::ProviderApiKey;
    let snapshot = readiness(
        &config,
        FeatureAvailability::Available,
        &full_text(true),
        true,
    );
    assert_eq!(chat_http(&snapshot).status, AiReadinessStatus::Ready);
}

/// A real old serialized setting has no surface identifier. It remains loadable,
/// but invocation evidence cannot repair the missing credential binding.
#[test]
fn legacy_saved_http_settings_request_resave_before_provider_verification() {
    let mut serialized = serde_json::to_value(http_config(Some("model-a"))).unwrap();
    serialized["ai_provider"]["llm_api"]
        .as_object_mut()
        .unwrap()
        .remove("surface_id");
    let mut legacy: AppConfig = serde_json::from_value(serialized).unwrap();
    assert!(legacy
        .ai_provider
        .llm_api
        .as_ref()
        .unwrap()
        .surface_id
        .is_none());
    for surface in [None, Some(""), Some("  ")] {
        legacy.ai_provider.llm_api.as_mut().unwrap().surface_id = surface.map(str::to_string);
        for verified in [false, true] {
            let snapshot = readiness(
                &legacy,
                FeatureAvailability::Available,
                &full_text(true),
                verified,
            );
            let chat = chat_http(&snapshot);
            assert_eq!(chat.status, AiReadinessStatus::Blocked);
            assert_eq!(
                chat.reason_code,
                AiReadinessReasonCode::HttpChatSettingsResaveRequired
            );
            assert_eq!(chat.action, AiReadinessAction::OpenAiSettings);
            assert!(chat.dimensions.http_chat_settings_resave_required);
            for capability in &snapshot.capabilities {
                if capability.capability_id != AiCapabilityId::ChatHttpApi {
                    assert!(!capability.dimensions.http_chat_settings_resave_required);
                }
            }
        }
    }
    let denied = readiness(
        &legacy,
        FeatureAvailability::Available,
        &full_text(false),
        true,
    );
    assert_eq!(
        chat_http(&denied).reason_code,
        AiReadinessReasonCode::ConsentRequired
    );
    assert_eq!(
        chat_http(&denied).action,
        AiReadinessAction::OpenPrivacyConsent
    );

    legacy.ai_provider.llm_api = None;
    let absent = readiness(
        &legacy,
        FeatureAvailability::Available,
        &full_text(true),
        true,
    );
    assert_eq!(
        chat_http(&absent).reason_code,
        AiReadinessReasonCode::EndpointOrProfileRequired
    );
    assert!(
        !chat_http(&absent)
            .dimensions
            .http_chat_settings_resave_required
    );
}

#[test]
fn resaving_a_legacy_endpoint_still_requires_verification_and_runtime_apply() {
    let mut boot = http_config(Some("model-a"));
    boot.ai_provider.llm_api.as_mut().unwrap().surface_id = None;
    let resaved = http_config(Some("model-a"));
    let unverified = readiness(
        &resaved,
        FeatureAvailability::Available,
        &full_text(true),
        false,
    );
    assert_eq!(
        chat_http(&unverified).reason_code,
        AiReadinessReasonCode::ProviderInvocationUnverified
    );
    assert!(
        !chat_http(&unverified)
            .dimensions
            .http_chat_settings_resave_required
    );
    let pending = build_ai_readiness_snapshot_with_local_preflight(
        &provider_snapshot(SURFACE, FeatureAvailability::Available, None),
        &resaved,
        &boot,
        &full_text(true),
        None,
        true,
    );
    assert_eq!(
        chat_http(&pending).reason_code,
        AiReadinessReasonCode::RestartRequired
    );
    let verified = readiness(
        &resaved,
        FeatureAvailability::Available,
        &full_text(true),
        true,
    );
    assert_eq!(chat_http(&verified).status, AiReadinessStatus::Ready);
}
