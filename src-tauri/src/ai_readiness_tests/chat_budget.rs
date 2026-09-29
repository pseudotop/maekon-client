use super::*;

#[test]
fn daily_token_budget_change_requires_chat_restart_only() {
    let provider = provider_snapshot(
        "provider_surface.openai.subprocess_cli",
        FeatureAvailability::Available,
        Some(ProviderCliReadiness::InvocationReady),
    );
    let consent = maekon_core::consent::ConsentPermissions {
        full_text_extraction: true,
        ..Default::default()
    };
    for (before, after) in [(0, 4096), (4096, 8192), (4096, 0)] {
        let mut boot = AppConfig::default_config();
        boot.ai_provider.access_mode = AiAccessMode::ProviderSubscriptionCli;
        boot.ai_provider.llm_api = Some(endpoint("provider_surface.openai.subprocess_cli"));
        boot.ai_session.daily_token_budget = before;
        let mut current = boot.clone();
        current.ai_session.daily_token_budget = after;
        let pending = build_ai_readiness_snapshot(&provider, &current, &boot, &consent);
        let restarted = build_ai_readiness_snapshot(&provider, &current, &current, &consent);

        for capability in [
            AiCapabilityId::ChatSubprocess,
            AiCapabilityId::ChatHttpApi,
            AiCapabilityId::ChatLocalLlm,
        ] {
            assert!(
                pending
                    .find(capability)
                    .expect("chat readiness")
                    .dimensions
                    .apply_pending
            );
            assert!(
                !restarted
                    .find(capability)
                    .expect("restarted chat")
                    .dimensions
                    .apply_pending
            );
        }
        assert_eq!(
            pending
                .find(AiCapabilityId::ChatSubprocess)
                .expect("subprocess chat")
                .reason_code,
            AiReadinessReasonCode::RestartRequired
        );
        assert_eq!(
            restarted
                .find(AiCapabilityId::ChatSubprocess)
                .expect("restarted subprocess chat")
                .status,
            AiReadinessStatus::Ready
        );
        for capability in [
            AiCapabilityId::OcrCapture,
            AiCapabilityId::OcrSuggestionAnalysis,
            AiCapabilityId::SegmentSummary,
            AiCapabilityId::DailyNarrative,
        ] {
            assert_eq!(
                pending
                    .find(capability)
                    .expect("non-chat readiness")
                    .dimensions
                    .apply_pending,
                restarted
                    .find(capability)
                    .expect("non-chat after restart")
                    .dimensions
                    .apply_pending,
                "chat budget must not change other pipelines' apply state"
            );
        }
    }
}
