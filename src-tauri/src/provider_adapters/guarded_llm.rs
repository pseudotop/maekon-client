use std::sync::Arc;

use async_trait::async_trait;
use maekon_core::error::CoreError;
use maekon_core::models::intent_execution::{
    IntentExecutionObserver, IntentProviderClass, IntentSkillContextKind, IntentSkillUse,
};
use maekon_core::ports::llm_provider::{
    InterpretedAction, LlmProvider, ScreenContext, SkillContext,
};

use super::types::{ensure_non_external_endpoints_are_loopback, ExternalOcrPrivacyGuard};

pub(super) struct GuardedLlmProvider {
    inner: Arc<dyn LlmProvider>,
    privacy_guard: ExternalOcrPrivacyGuard,
}

impl GuardedLlmProvider {
    pub(super) fn new(inner: Arc<dyn LlmProvider>, privacy_guard: ExternalOcrPrivacyGuard) -> Self {
        Self {
            inner,
            privacy_guard,
        }
    }
}

#[async_trait]
impl LlmProvider for GuardedLlmProvider {
    async fn interpret_intent_observed(
        &self,
        screen_context: &ScreenContext,
        intent_hint: &str,
        skills: Option<&SkillContext>,
        observer: &IntentExecutionObserver,
    ) -> Result<InterpretedAction, CoreError> {
        let prepared = if self.inner.is_external() {
            self.privacy_guard
                .prepare_screen_context_for_external_llm(screen_context, self.inner.provider_name())
                .await
        } else {
            ensure_non_external_endpoints_are_loopback(
                self.inner.provider_name(),
                self.inner.egress_endpoint_urls(),
            )
            .map(|()| screen_context.clone())
        };
        match prepared {
            Ok(context) => {
                self.inner
                    .interpret_intent_observed(&context, intent_hint, skills, observer)
                    .await
            }
            Err(error) => {
                // The guard denied the leg before any context reached its provider.
                let mut capabilities = self.inner.intent_capability_observation(skills);
                capabilities.skills_context_forwarded = Some(false);
                capabilities.skill_context_kind = IntentSkillContextKind::None;
                capabilities.skills_effective = IntentSkillUse::NotUsed;
                let attempt =
                    observer.begin_attempt(self.inner.intent_provider_class(), capabilities);
                observer.end_attempt(attempt, Err(&error));
                Err(error)
            }
        }
    }

    fn intent_provider_class(&self) -> IntentProviderClass {
        self.inner.intent_provider_class()
    }

    fn intent_requested_provider_class(&self) -> IntentProviderClass {
        self.inner.intent_requested_provider_class()
    }

    fn intent_forwards_skill_context(&self) -> Option<bool> {
        self.inner.intent_forwards_skill_context()
    }

    async fn interpret_intent(
        &self,
        screen_context: &ScreenContext,
        intent_hint: &str,
    ) -> Result<InterpretedAction, CoreError> {
        if !self.inner.is_external() {
            ensure_non_external_endpoints_are_loopback(
                self.inner.provider_name(),
                self.inner.egress_endpoint_urls(),
            )?;
            return self
                .inner
                .interpret_intent(screen_context, intent_hint)
                .await;
        }

        let sanitized = self
            .privacy_guard
            .prepare_screen_context_for_external_llm(screen_context, self.inner.provider_name())
            .await?;
        self.inner.interpret_intent(&sanitized, intent_hint).await
    }

    async fn interpret_intent_with_skills(
        &self,
        screen_context: &ScreenContext,
        intent_hint: &str,
        skill_ctx: &SkillContext,
    ) -> Result<InterpretedAction, CoreError> {
        if !self.inner.is_external() {
            ensure_non_external_endpoints_are_loopback(
                self.inner.provider_name(),
                self.inner.egress_endpoint_urls(),
            )?;
            return self
                .inner
                .interpret_intent_with_skills(screen_context, intent_hint, skill_ctx)
                .await;
        }

        let sanitized = self
            .privacy_guard
            .prepare_screen_context_for_external_llm(screen_context, self.inner.provider_name())
            .await?;
        self.inner
            .interpret_intent_with_skills(&sanitized, intent_hint, skill_ctx)
            .await
    }

    fn provider_name(&self) -> &str {
        self.inner.provider_name()
    }

    fn is_external(&self) -> bool {
        self.inner.is_external()
    }

    fn egress_endpoint_urls(&self) -> Vec<&str> {
        self.inner.egress_endpoint_urls()
    }
}

#[cfg(test)]
mod observation_tests {
    use super::*;
    use maekon_core::config::{ExternalDataPolicy, PiiFilterLevel, PrivacyConfig};
    use maekon_core::consent::{ConsentManager, ConsentPermissions};
    use maekon_core::models::context::{ProcessInfo, WindowInfo};
    use maekon_core::models::event::ProcessDetail;
    use maekon_core::models::intent_execution::{IntentAttemptOutcome, IntentSubmission};
    use maekon_core::models::skill::SkillMeta;
    use maekon_core::ports::monitor::ProcessMonitor;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    struct SyntheticMonitor;
    #[async_trait]
    impl ProcessMonitor for SyntheticMonitor {
        async fn get_active_window(&self) -> Result<Option<WindowInfo>, CoreError> {
            Ok(Some(WindowInfo {
                title: "CANARY_WINDOW".into(),
                app_name: "Code".into(),
                app_bundle_id: None,
                pid: 1,
                bounds: None,
            }))
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

    struct SyntheticProvider {
        calls: Arc<AtomicUsize>,
        external: bool,
    }

    struct ReceivedCall {
        screen: ScreenContext,
        hint: String,
        skills: Option<SkillContext>,
    }

    struct RecordingProvider {
        calls: Arc<Mutex<Vec<ReceivedCall>>>,
        external: bool,
    }

    impl RecordingProvider {
        fn record(
            &self,
            screen: &ScreenContext,
            hint: &str,
            skills: Option<&SkillContext>,
        ) -> Result<InterpretedAction, CoreError> {
            self.calls.lock().unwrap().push(ReceivedCall {
                screen: screen.clone(),
                hint: hint.into(),
                skills: skills.cloned(),
            });
            Ok(InterpretedAction {
                target_text: Some("save".into()),
                target_role: None,
                action_type: "click".into(),
                confidence: 0.9,
            })
        }
    }

    #[async_trait]
    impl LlmProvider for RecordingProvider {
        async fn interpret_intent(
            &self,
            screen: &ScreenContext,
            hint: &str,
        ) -> Result<InterpretedAction, CoreError> {
            self.record(screen, hint, None)
        }
        async fn interpret_intent_with_skills(
            &self,
            screen: &ScreenContext,
            hint: &str,
            skills: &SkillContext,
        ) -> Result<InterpretedAction, CoreError> {
            self.record(screen, hint, Some(skills))
        }
        fn provider_name(&self) -> &str {
            "recording-provider"
        }
        fn is_external(&self) -> bool {
            self.external
        }
        fn egress_endpoint_urls(&self) -> Vec<&str> {
            vec!["http://127.0.0.1:11434/v1/responses"]
        }
        fn intent_provider_class(&self) -> IntentProviderClass {
            IntentProviderClass::RemoteModel
        }
        fn intent_requested_provider_class(&self) -> IntentProviderClass {
            IntentProviderClass::LocalModel
        }
        fn intent_forwards_skill_context(&self) -> Option<bool> {
            Some(true)
        }
    }

    #[tokio::test]
    async fn observed_guard_preserves_real_sanitization_inputs_and_legacy_results() {
        for external in [true, false] {
            for with_skills in [true, false] {
                let temp = tempfile::tempdir().unwrap();
                let consent = Arc::new(ConsentManager::new(temp.path().join("consent.json")));
                consent
                    .grant_consent(
                        ConsentPermissions {
                            full_text_extraction: true,
                            ..Default::default()
                        },
                        30,
                    )
                    .unwrap();
                let privacy = ExternalOcrPrivacyGuard::new(
                    consent,
                    PiiFilterLevel::Standard,
                    ExternalDataPolicy::PiiFilterStandard,
                    PrivacyConfig::default(),
                    Arc::new(SyntheticMonitor),
                    None,
                );
                let calls = Arc::new(Mutex::new(Vec::new()));
                let guarded = GuardedLlmProvider::new(
                    Arc::new(RecordingProvider {
                        calls: calls.clone(),
                        external,
                    }),
                    privacy,
                );
                assert_eq!(
                    guarded.intent_provider_class(),
                    IntentProviderClass::RemoteModel
                );
                assert_eq!(
                    guarded.intent_requested_provider_class(),
                    IntentProviderClass::LocalModel
                );
                assert_eq!(guarded.intent_forwards_skill_context(), Some(true));
                let context = ScreenContext {
                    visible_texts: vec!["canary.person@example.com".into()],
                    active_app: "Code".into(),
                    active_window_title: "canary.person@example.com".into(),
                    layout_description: Some("canary.person@example.com".into()),
                };
                let skills = with_skills.then_some(SkillContext {
                    available_skills: vec![SkillMeta {
                        name: "CANARY_SKILL".into(),
                        description: "CANARY_SKILL_BODY".into(),
                    }],
                    active_skill: None,
                });
                let legacy = match &skills {
                    Some(skills) => {
                        guarded
                            .interpret_intent_with_skills(&context, "CANARY_HINT", skills)
                            .await
                    }
                    None => guarded.interpret_intent(&context, "CANARY_HINT").await,
                }
                .unwrap();
                let observer = IntentExecutionObserver::new(IntentSubmission::default());
                let observed = guarded
                    .interpret_intent_observed(&context, "CANARY_HINT", skills.as_ref(), &observer)
                    .await
                    .unwrap();
                assert_eq!(
                    serde_json::to_value(legacy).unwrap(),
                    serde_json::to_value(observed).unwrap()
                );
                let received = calls.lock().unwrap();
                assert_eq!(received.len(), 2);
                assert_eq!(
                    serde_json::to_value(&received[0].screen).unwrap(),
                    serde_json::to_value(&received[1].screen).unwrap()
                );
                for call in received.iter() {
                    assert_eq!(call.hint, "CANARY_HINT");
                    assert_eq!(call.skills.is_some(), with_skills);
                    if let Some(skills) = &call.skills {
                        assert_eq!(skills.available_skills[0].name, "CANARY_SKILL");
                    }
                    let screen = serde_json::to_string(&call.screen).unwrap();
                    assert_eq!(screen.contains("canary.person@example.com"), !external);
                }
                drop(received);
                let receipt = observer.snapshot();
                assert_eq!(receipt.attempts.len(), 1);
                assert_eq!(receipt.attempts[0].outcome, IntentAttemptOutcome::Succeeded);
                assert_eq!(
                    receipt.attempts[0].effective_provider_class,
                    IntentProviderClass::RemoteModel
                );
                assert_eq!(
                    receipt.attempts[0].capabilities.skills_context_forwarded,
                    Some(with_skills)
                );
                assert_eq!(
                    receipt.attempts[0].capabilities.skills_effective,
                    IntentSkillUse::Unknown
                );
                assert!(!serde_json::to_string(&receipt).unwrap().contains("CANARY"));
                assert!(!serde_json::to_string(&receipt)
                    .unwrap()
                    .contains("canary.person"));
            }
        }
    }
    #[async_trait]
    impl LlmProvider for SyntheticProvider {
        async fn interpret_intent(
            &self,
            _: &ScreenContext,
            _: &str,
        ) -> Result<InterpretedAction, CoreError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            panic!("the denied observed leg must never reach its provider")
        }
        fn provider_name(&self) -> &str {
            "synthetic"
        }
        fn is_external(&self) -> bool {
            self.external
        }
        fn egress_endpoint_urls(&self) -> Vec<&str> {
            vec!["https://example.invalid/v1"]
        }
        fn intent_provider_class(&self) -> IntentProviderClass {
            IntentProviderClass::RemoteModel
        }
        fn intent_forwards_skill_context(&self) -> Option<bool> {
            Some(true)
        }
    }

    #[tokio::test]
    async fn guarded_local_rule_keeps_unsupported_skills_without_external_consent() {
        let temp = tempfile::tempdir().unwrap();
        let guard = ExternalOcrPrivacyGuard::new(
            Arc::new(ConsentManager::new(temp.path().join("absent-consent.json"))),
            PiiFilterLevel::Standard,
            ExternalDataPolicy::PiiFilterStandard,
            PrivacyConfig::default(),
            Arc::new(SyntheticMonitor),
            None,
        );
        let provider = GuardedLlmProvider::new(
            Arc::new(maekon_automation::local_llm::LocalLlmProvider::new()),
            guard,
        );
        assert_eq!(provider.intent_forwards_skill_context(), Some(false));
        let observer = IntentExecutionObserver::new(IntentSubmission::default());
        let screen = ScreenContext {
            visible_texts: vec![],
            active_app: "Code".into(),
            active_window_title: String::new(),
            layout_description: None,
        };
        let skills = SkillContext {
            available_skills: vec![SkillMeta {
                name: "CANARY_SKILL".into(),
                description: "CANARY_BODY".into(),
            }],
            active_skill: None,
        };
        provider
            .interpret_intent_observed(&screen, "save", Some(&skills), &observer)
            .await
            .unwrap();
        let receipt = observer.snapshot();
        assert_eq!(receipt.attempts.len(), 1);
        assert_eq!(
            receipt.attempts[0].effective_provider_class,
            IntentProviderClass::Rule
        );
        assert_eq!(
            receipt.attempts[0].capabilities.skills_context_forwarded,
            Some(false)
        );
        assert_eq!(
            receipt.attempts[0].capabilities.skills_effective,
            IntentSkillUse::Unsupported
        );
    }

    #[tokio::test]
    async fn observed_calls_preserve_consent_and_non_loopback_denials_without_egress() {
        for external in [true, false] {
            let temp = tempfile::tempdir().unwrap();
            let privacy = ExternalOcrPrivacyGuard::new(
                Arc::new(ConsentManager::new(temp.path().join("absent-consent.json"))),
                PiiFilterLevel::Standard,
                ExternalDataPolicy::PiiFilterStandard,
                PrivacyConfig::default(),
                Arc::new(SyntheticMonitor),
                None,
            );
            let calls = Arc::new(AtomicUsize::new(0));
            let guarded = GuardedLlmProvider::new(
                Arc::new(SyntheticProvider {
                    calls: calls.clone(),
                    external,
                }),
                privacy,
            );
            let observer = IntentExecutionObserver::new(IntentSubmission::default());
            let context = ScreenContext {
                visible_texts: vec!["CANARY_OCR".into()],
                active_app: "Code".into(),
                active_window_title: "CANARY_WINDOW".into(),
                layout_description: None,
            };
            let error = guarded
                .interpret_intent_observed(
                    &context,
                    "CANARY_PROMPT",
                    Some(&SkillContext::default()),
                    &observer,
                )
                .await
                .unwrap_err();
            assert!(matches!(error, CoreError::PolicyDenied { .. }));
            assert_eq!(calls.load(Ordering::SeqCst), 0);
            let receipt = observer.snapshot();
            assert_eq!(receipt.attempts.len(), 1);
            assert_eq!(receipt.attempts[0].outcome, IntentAttemptOutcome::Denied);
            assert_eq!(
                receipt.attempts[0].capabilities.skills_context_forwarded,
                Some(false)
            );
            assert_eq!(
                receipt.attempts[0].capabilities.skills_effective,
                IntentSkillUse::NotUsed
            );
            assert!(!serde_json::to_string(&receipt).unwrap().contains("CANARY"));
        }
    }
}
