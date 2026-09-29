use super::*;
use maekon_core::ports::secret_store::SecretStore;

#[derive(Default)]
struct Secrets(AtomicUsize);
#[async_trait]
impl SecretStore for Secrets {
    async fn retrieve(&self, _namespace: &str, _key: &str) -> Result<Option<String>, CoreError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        panic!("local/off routing must never query secrets");
    }
    async fn store(&self, _namespace: &str, _key: &str, _value: &str) -> Result<(), CoreError> {
        unreachable!()
    }
    async fn delete(&self, _namespace: &str, _key: &str) -> Result<(), CoreError> {
        unreachable!()
    }
    async fn delete_namespace(&self, _namespace: &str) -> Result<(), CoreError> {
        unreachable!()
    }
}

#[tokio::test]
async fn local_assessment_factory_isolates_off_local_and_unsupported_providers_from_keys_and_http()
{
    use crate::provider_adapters::{build_candidate_assessment_runtime, AssessmentRuntimeControl};
    let f = fixture(CandidateProvider::LocalRules, 10);
    let secrets = Arc::new(Secrets::default());
    for provider in [
        None,
        Some(CandidateProvider::LocalRules),
        Some(CandidateProvider::LocalModel),
        Some(CandidateProvider::JevDirect),
        Some(CandidateProvider::CodexCli),
        Some(CandidateProvider::ClaudeCli),
        Some(CandidateProvider::JevGateway),
    ] {
        let audit_dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(
            maekon_storage::sqlite::SqliteStorage::open(
                &audit_dir.path().join("audit.db"),
                30,
                None,
            )
            .unwrap(),
        );
        let built = build_candidate_assessment_runtime(
            CandidateDecisionPolicy {
                provider,
                ..CandidateDecisionPolicy::default()
            },
            AiAccessMode::LocalModel,
            f.runtime.privacy.clone(),
            storage,
            Some("this is not a URL"),
            Some(secrets.clone()),
        );
        if provider == Some(CandidateProvider::LocalRules) {
            let AssessmentRuntimeControl::Local(control) = &built.control else {
                panic!("explicit local rules must be constructible without an endpoint or key");
            };
            unavailable(
                &built.provider.assess(&f.request).await,
                DecisionUnavailable::Off,
            );
            control.bind(&f.request).unwrap();
            control
                .approve(approval(CandidateProvider::LocalRules, 10))
                .unwrap();
            selected(&built.provider.assess(&f.request).await, "private-id-0");
        } else {
            assert!(matches!(built.control, AssessmentRuntimeControl::Off));
            let expected = match provider {
                None => DecisionUnavailable::Off,
                #[cfg(feature = "analysis")]
                Some(CandidateProvider::LocalModel) => DecisionUnavailable::Transport,
                #[cfg(feature = "analysis")]
                Some(CandidateProvider::JevDirect) => DecisionUnavailable::LocalOnly,
                _ => DecisionUnavailable::Rejected,
            };
            unavailable(&built.provider.assess(&f.request).await, expected);
        }
    }
    assert_eq!(secrets.0.load(Ordering::SeqCst), 0);
}

#[cfg(feature = "analysis")]
#[tokio::test]
async fn local_assessment_factory_composes_loopback_inference_and_durable_audit() {
    use crate::provider_adapters::{build_candidate_assessment_runtime, AssessmentRuntimeControl};
    let f = fixture(CandidateProvider::LocalRules, 10);
    let secrets = Arc::new(Secrets::default());
    let audit_dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(
        maekon_storage::sqlite::SqliteStorage::open(&audit_dir.path().join("audit.db"), 30, None)
            .unwrap(),
    );
    let mut server = mockito::Server::new_async().await;
    let mut approved = approval(CandidateProvider::LocalModel, 1);
    approved.model.as_mut().unwrap().endpoint_origin = server.url();
    let status = server
        .mock("GET", "/api/status")
        .with_status(200)
        .with_body(r#"{"cloud":{"disabled":true,"source":"env"}}"#)
        .expect(2)
        .create_async()
        .await;
    let tags = server
        .mock("GET", "/api/tags")
        .with_status(200)
        .with_body(
            serde_json::json!({"models":[{
                "name":"fixture:fixed",
                "model":"fixture:fixed",
                "digest":"a".repeat(64)
            }]})
            .to_string(),
        )
        .expect(2)
        .create_async()
        .await;
    let chat = server
        .mock("POST", "/api/chat")
        .match_header("authorization", mockito::Matcher::Missing)
        .match_body(mockito::Matcher::PartialJson(serde_json::json!({
            "model":"fixture:fixed","stream":false,"think":false,
            "format":{"properties":{"selected":{"enum":["c0","none","delegate"]}}}
        })))
        .with_status(200)
        .with_body(
            serde_json::json!({
                "model":"fixture:fixed","done":true,
                "message":{"role":"assistant","content":r#"{"selected":"c0"}"#},
                "prompt_eval_count":14,"eval_count":3
            })
            .to_string(),
        )
        .expect(1)
        .create_async()
        .await;
    let built = build_candidate_assessment_runtime(
        CandidateDecisionPolicy {
            provider: Some(CandidateProvider::LocalModel),
            ..CandidateDecisionPolicy::default()
        },
        AiAccessMode::LocalModel,
        f.runtime.privacy.clone(),
        storage.clone(),
        Some(&server.url()),
        Some(secrets.clone()),
    );
    let AssessmentRuntimeControl::Local(control) = &built.control else {
        panic!("valid local model must be constructible");
    };
    control.bind(&f.request).unwrap();
    control.approve(approved).unwrap();
    let result = built.provider.assess(&f.request).await;
    selected(&result, "private-id-0");
    assert!(matches!(result.evidence, AssessmentEvidence::ModelReported));
    assert_eq!(
        result.provenance.unwrap().transport,
        AssessmentTransport::LoopbackHttp
    );
    assert_eq!(storage.verify_audit_chain().verified_count, 2);
    assert!(storage.verify_audit_chain().ok);
    assert_eq!(secrets.0.load(Ordering::SeqCst), 0);
    unavailable(
        &built.provider.assess(&f.request).await,
        DecisionUnavailable::BudgetExceeded,
    );
    assert_eq!(storage.verify_audit_chain().verified_count, 2);
    status.assert_async().await;
    tags.assert_async().await;
    chat.assert_async().await;
}

#[cfg(feature = "analysis")]
#[tokio::test]
async fn local_assessment_factory_keeps_missing_endpoint_and_jev_approval_inert() {
    use crate::provider_adapters::{build_candidate_assessment_runtime, AssessmentRuntimeControl};
    let f = fixture(CandidateProvider::LocalRules, 10);
    let storage = Arc::new(maekon_storage::sqlite::SqliteStorage::open_in_memory(30).unwrap());
    let secrets = Arc::new(Secrets::default());
    let model = build_candidate_assessment_runtime(
        CandidateDecisionPolicy {
            provider: Some(CandidateProvider::LocalModel),
            ..CandidateDecisionPolicy::default()
        },
        AiAccessMode::LocalModel,
        f.runtime.privacy.clone(),
        storage.clone(),
        None,
        Some(secrets.clone()),
    );
    assert!(matches!(model.control, AssessmentRuntimeControl::Off));
    unavailable(
        &model.provider.assess(&f.request).await,
        DecisionUnavailable::Rejected,
    );

    let paid = CandidateDecisionPolicy {
        provider: Some(CandidateProvider::JevDirect),
        cost: CandidateCostPolicy::ExplicitPaidApiAllowed,
    };
    let missing = build_candidate_assessment_runtime(
        paid,
        AiAccessMode::ProviderApiKey,
        f.runtime.privacy.clone(),
        storage.clone(),
        Some("this is not a URL"),
        None,
    );
    assert!(matches!(missing.control, AssessmentRuntimeControl::Off));
    unavailable(
        &missing.provider.assess(&f.request).await,
        DecisionUnavailable::CredentialUnavailable,
    );

    let built = build_candidate_assessment_runtime(
        paid,
        AiAccessMode::ProviderApiKey,
        f.runtime.privacy.clone(),
        storage.clone(),
        Some("this is not a URL"),
        Some(secrets.clone()),
    );
    assert!(matches!(built.control, AssessmentRuntimeControl::Jev(_)));
    let result = built.provider.assess(&f.request).await;
    unavailable(&result, DecisionUnavailable::Off);
    assert!(result.attempts.is_empty());
    assert_eq!(secrets.0.load(Ordering::SeqCst), 0);
    assert_eq!(storage.verify_audit_chain().verified_count, 0);
}
