#![cfg(test)]

use super::*;

#[test]
fn selected_invocation_ready_cli_requires_full_text_consent_and_tracks_withdrawal() {
    let provider = provider_snapshot(
        "provider_surface.openai.subprocess_cli",
        FeatureAvailability::Available,
        Some(ProviderCliReadiness::InvocationReady),
    );
    let mut config = AppConfig::default_config();
    config.ai_provider.access_mode = AiAccessMode::ProviderSubscriptionCli;
    config.ai_provider.llm_api = Some(endpoint("provider_surface.openai.subprocess_cli"));

    for granted in [false, true, false] {
        let permissions = maekon_core::consent::ConsentPermissions {
            full_text_extraction: granted,
            ..Default::default()
        };
        let readiness = build_ai_readiness_snapshot(&provider, &config, &config, &permissions);
        let chat = readiness
            .find(AiCapabilityId::ChatSubprocess)
            .expect("subprocess chat readiness");
        assert_eq!(
            chat.dimensions.consent,
            vec![AiConsentReadiness {
                field: AiConsentField::FullTextExtraction,
                granted,
            }]
        );
        assert_eq!(
            chat.status,
            if granted {
                AiReadinessStatus::Ready
            } else {
                AiReadinessStatus::Blocked
            }
        );
        assert_eq!(
            chat.reason_code,
            if granted {
                AiReadinessReasonCode::Ready
            } else {
                AiReadinessReasonCode::ConsentRequired
            }
        );
        if !granted {
            assert_eq!(
                chat.action,
                maekon_core::ai_readiness::AiReadinessAction::OpenPrivacyConsent
            );
            let mut missing_dimension = chat.dimensions.clone();
            missing_dimension.consent.clear();
            assert_eq!(
                evaluate_ai_readiness(AiCapabilityId::ChatSubprocess, missing_dimension).status,
                AiReadinessStatus::Ready,
                "removing the consent dimension must reproduce the misleading ready state"
            );
        }
        assert_eq!(readiness.capabilities.len(), 7);
    }
}

#[test]
fn configured_http_is_unverified_until_a_model_invocation_succeeds() {
    let provider = provider_snapshot(
        "provider_surface.fixture.direct_http",
        FeatureAvailability::Available,
        None,
    );
    let mut config = AppConfig::default_config();
    config.ai_provider.access_mode = AiAccessMode::ProviderApiKey;
    config.ai_provider.llm_api = Some(endpoint("provider_surface.fixture.direct_http"));

    let denied = build_ai_readiness_snapshot(&provider, &config, &config, &Default::default());
    let denied_chat = denied
        .find(AiCapabilityId::ChatHttpApi)
        .expect("HTTP chat readiness");
    assert_eq!(
        denied_chat.reason_code,
        AiReadinessReasonCode::ConsentRequired
    );
    assert_eq!(
        denied_chat.action,
        maekon_core::ai_readiness::AiReadinessAction::OpenPrivacyConsent
    );

    let readiness = build_ai_readiness_snapshot(
        &provider,
        &config,
        &config,
        &maekon_core::consent::ConsentPermissions {
            full_text_extraction: true,
            ..Default::default()
        },
    );

    let chat = readiness
        .find(AiCapabilityId::ChatHttpApi)
        .expect("HTTP chat readiness");
    assert_eq!(chat.status, AiReadinessStatus::Blocked);
    assert_eq!(
        chat.reason_code,
        AiReadinessReasonCode::ProviderInvocationUnverified
    );
    assert_eq!(
        chat.dimensions.model_availability,
        AiModelAvailability::Unverified
    );
}
