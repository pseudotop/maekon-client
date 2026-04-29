use async_trait::async_trait;
use oneshim_automation::input_driver::{NoOpElementFinder, NoOpInputDriver};
use oneshim_automation::intent_planner::LlmIntentPlanner;
use oneshim_automation::intent_resolver::{IntentExecutor, IntentResolver};
use oneshim_core::config::{AiAccessMode, AiProviderConfig, PiiFilterLevel};
use oneshim_core::error::CoreError;
use oneshim_core::models::intent::{ElementBounds, IntentConfig, UiElement};
use oneshim_core::models::ui_scene::UiScene;
use oneshim_core::ports::element_finder::ElementFinder;
use oneshim_core::ports::input_driver::InputDriver;
use oneshim_core::ports::intent_planner::IntentPlanner;
use oneshim_core::ports::secret_store::SecretStoreSet;
use oneshim_core::ports::skill_loader::SkillLoader;
use oneshim_storage::frame_storage::FrameFileStorage;
use oneshim_vision::element_finder::OcrElementFinder;
use std::sync::Arc;
use tracing::{debug, warn};

use crate::platform_accessibility::create_platform_accessibility_finder;
use crate::provider_adapters::{
    resolve_ai_provider_adapters, ExternalOcrPrivacyGuard, ProviderSource,
};
#[cfg(feature = "server")]
use oneshim_core::ports::oauth::OAuthPort;

pub struct AutomationRuntime {
    pub element_finder: Arc<dyn ElementFinder>,
    pub intent_executor: Arc<IntentExecutor>,
    pub intent_planner: Arc<dyn IntentPlanner>,
    pub access_mode: AiAccessMode,
    pub ocr_provider_name: String,
    pub llm_provider_name: String,
    pub ocr_source: ProviderSource,
    pub llm_source: ProviderSource,
    pub ocr_fallback_reason: Option<String>,
    pub llm_fallback_reason: Option<String>,
}

pub struct CompositeElementFinder {
    finders: Vec<Arc<dyn ElementFinder>>,
}

impl CompositeElementFinder {
    pub fn new(finders: Vec<Arc<dyn ElementFinder>>) -> Self {
        Self { finders }
    }
}

#[async_trait]
impl ElementFinder for CompositeElementFinder {
    async fn find_element(
        &self,
        text: Option<&str>,
        role: Option<&str>,
        region: Option<&ElementBounds>,
    ) -> Result<Vec<UiElement>, CoreError> {
        let mut last_err: Option<CoreError> = None;
        for finder in &self.finders {
            debug!(finder = finder.name(), "composite finder: find_element");
            match finder.find_element(text, role, region).await {
                Ok(elements) if !elements.is_empty() => return Ok(elements),
                Ok(_) => continue,
                Err(err) => last_err = Some(err),
            }
        }
        Err(last_err.unwrap_or_else(|| CoreError::ElementNotFound {
            code: oneshim_core::error_codes::UiCode::ElementMissing,
            name: "No element found by any configured finder".to_string(),
        }))
    }

    async fn analyze_scene(
        &self,
        app_name: Option<&str>,
        screen_id: Option<&str>,
    ) -> Result<UiScene, CoreError> {
        let mut last_err: Option<CoreError> = None;
        for finder in &self.finders {
            debug!(finder = finder.name(), "composite finder: analyze_scene");
            match finder.analyze_scene(app_name, screen_id).await {
                Ok(scene) => return Ok(scene),
                Err(err) => last_err = Some(err),
            }
        }

        Err(last_err.unwrap_or_else(|| CoreError::ElementNotFound {
            code: oneshim_core::error_codes::UiCode::ElementMissing,
            name: "No scene produced by any configured finder".to_string(),
        }))
    }

    async fn analyze_scene_from_image(
        &self,
        image_data: Vec<u8>,
        image_format: String,
        app_name: Option<&str>,
        screen_id: Option<&str>,
    ) -> Result<UiScene, CoreError> {
        let mut last_err: Option<CoreError> = None;
        for finder in &self.finders {
            debug!(
                finder = finder.name(),
                "composite finder: analyze_scene_from_image"
            );
            match finder
                .analyze_scene_from_image(
                    image_data.clone(),
                    image_format.clone(),
                    app_name,
                    screen_id,
                )
                .await
            {
                Ok(scene) => return Ok(scene),
                Err(err) => last_err = Some(err),
            }
        }

        Err(last_err.unwrap_or_else(|| CoreError::ElementNotFound {
            code: oneshim_core::error_codes::UiCode::ElementMissing,
            name: "No image scene produced by any configured finder".to_string(),
        }))
    }

    fn name(&self) -> &str {
        "composite"
    }
}

pub fn build_automation_runtime(
    ai_config: &AiProviderConfig,
    pii_filter_level: PiiFilterLevel,
    frame_storage: Option<Arc<FrameFileStorage>>,
    external_ocr_privacy_guard: Option<ExternalOcrPrivacyGuard>,
    skill_loader: Option<Arc<dyn SkillLoader>>,
    secret_stores: Option<SecretStoreSet>,
    #[cfg(feature = "server")] oauth_port: Option<Arc<dyn OAuthPort>>,
) -> Result<AutomationRuntime, CoreError> {
    let adapters = resolve_ai_provider_adapters(
        ai_config,
        pii_filter_level,
        external_ocr_privacy_guard,
        secret_stores,
        #[cfg(feature = "server")]
        oauth_port,
    )?;

    let ocr_provider_name = adapters.ocr.provider_name().to_string();
    let llm_provider_name = adapters.llm.provider_name().to_string();

    #[cfg(feature = "native-vision")]
    let rect_detector: Option<
        Arc<dyn oneshim_core::ports::rectangle_detector::RectangleDetector>,
    > = oneshim_vision::native_detect::create_rectangle_detector();
    #[cfg(not(feature = "native-vision"))]
    let rect_detector: Option<
        Arc<dyn oneshim_core::ports::rectangle_detector::RectangleDetector>,
    > = None;

    let ocr_finder: Arc<dyn ElementFinder> = if let Some(frame_storage) = frame_storage {
        let finder = LatestFrameOcrElementFinder::new(frame_storage, adapters.ocr.clone());
        let finder = if let Some(det) = rect_detector {
            finder.with_rectangle_detector(det)
        } else {
            finder
        };
        Arc::new(finder)
    } else {
        warn!("frame save settings: NoOpElementFinder");
        Arc::new(NoOpElementFinder)
    };

    let accessibility_finder = create_platform_accessibility_finder();
    let element_finder: Arc<dyn ElementFinder> = Arc::new(CompositeElementFinder::new(vec![
        accessibility_finder,
        ocr_finder,
    ]));

    let input_driver: Arc<dyn InputDriver> = Arc::new(NoOpInputDriver);
    let resolver = IntentResolver::new(
        element_finder.clone(),
        input_driver,
        IntentConfig::default(),
    );
    let intent_executor = Arc::new(IntentExecutor::new(resolver, IntentConfig::default()));
    let planner = LlmIntentPlanner::new(adapters.llm.clone(), element_finder.clone());
    let intent_planner: Arc<dyn IntentPlanner> = if let Some(loader) = skill_loader {
        Arc::new(planner.with_skill_loader(loader))
    } else {
        Arc::new(planner)
    };

    Ok(AutomationRuntime {
        element_finder,
        intent_executor,
        intent_planner,
        access_mode: ai_config.access_mode,
        ocr_provider_name,
        llm_provider_name,
        ocr_source: adapters.ocr_source,
        llm_source: adapters.llm_source,
        ocr_fallback_reason: adapters.ocr_fallback_reason,
        llm_fallback_reason: adapters.llm_fallback_reason,
    })
}

pub fn build_noop_intent_executor() -> Arc<IntentExecutor> {
    let input_driver: Arc<dyn InputDriver> = Arc::new(NoOpInputDriver);
    let element_finder: Arc<dyn ElementFinder> = Arc::new(NoOpElementFinder);
    let resolver = IntentResolver::new(element_finder, input_driver, IntentConfig::default());
    Arc::new(IntentExecutor::new(resolver, IntentConfig::default()))
}

pub struct LatestFrameOcrElementFinder {
    frame_storage: Arc<FrameFileStorage>,
    inner: OcrElementFinder,
}

impl LatestFrameOcrElementFinder {
    pub fn new(
        frame_storage: Arc<FrameFileStorage>,
        ocr_provider: Arc<dyn oneshim_core::ports::ocr_provider::OcrProvider>,
    ) -> Self {
        Self {
            frame_storage,
            inner: OcrElementFinder::new(ocr_provider),
        }
    }

    pub fn with_rectangle_detector(
        mut self,
        detector: Arc<dyn oneshim_core::ports::rectangle_detector::RectangleDetector>,
    ) -> Self {
        self.inner = self.inner.with_rectangle_detector(detector);
        self
    }

    async fn refresh_latest_frame(&self) -> Result<bool, CoreError> {
        match self.frame_storage.load_latest_frame().await? {
            Some((image_data, image_format)) => {
                self.inner.set_image(image_data, image_format).await;
                Ok(true)
            }
            None => Ok(false),
        }
    }
}

#[async_trait]
impl ElementFinder for LatestFrameOcrElementFinder {
    async fn find_element(
        &self,
        text: Option<&str>,
        role: Option<&str>,
        region: Option<&ElementBounds>,
    ) -> Result<Vec<UiElement>, CoreError> {
        if !self.refresh_latest_frame().await? {
            return Err(CoreError::ElementNotFound {
                code: oneshim_core::error_codes::UiCode::ElementMissing,
                name: "자동화용 최신 frame이 없습니다".to_string(),
            });
        }
        self.inner.find_element(text, role, region).await
    }

    async fn analyze_scene(
        &self,
        app_name: Option<&str>,
        screen_id: Option<&str>,
    ) -> Result<UiScene, CoreError> {
        if !self.refresh_latest_frame().await? {
            return Err(CoreError::ElementNotFound {
                code: oneshim_core::error_codes::UiCode::ElementMissing,
                name: "자동화용 최신 frame이 없습니다".to_string(),
            });
        }
        self.inner
            .analyze_scene(app_name, screen_id)
            .await
            .map_err(Into::into)
    }

    async fn analyze_scene_from_image(
        &self,
        image_data: Vec<u8>,
        image_format: String,
        app_name: Option<&str>,
        screen_id: Option<&str>,
    ) -> Result<UiScene, CoreError> {
        self.inner
            .analyze_scene_from_image(image_data, image_format, app_name, screen_id)
            .await
    }

    fn name(&self) -> &str {
        "latest-frame-ocr"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "server")]
    use async_trait::async_trait;
    use chrono::Utc;
    #[cfg(feature = "server")]
    use oneshim_core::config::{
        AiAccessMode, AiProviderType, CredentialAuthMode, CredentialBackendKind, CredentialBinding,
        ExternalApiEndpoint, PrivacyConfig, SecretRef,
    };
    use oneshim_core::config::{AiProviderConfig, LlmProviderType, OcrProviderType};
    #[cfg(feature = "server")]
    use oneshim_core::consent::{ConsentManager, ConsentPermissions};
    #[cfg(feature = "server")]
    use oneshim_core::models::context::{ProcessInfo, WindowInfo};
    #[cfg(feature = "server")]
    use oneshim_core::models::event::ProcessDetail;
    #[cfg(feature = "server")]
    use oneshim_core::ports::monitor::ProcessMonitor;
    use oneshim_core::ports::ocr_provider::{OcrProvider, OcrResult};
    #[cfg(feature = "server")]
    use oneshim_core::ports::secret_store::{
        provider_api_key_secret_ref, secret_env_var_name, SecretStoreSet,
    };
    #[cfg(feature = "server")]
    use oneshim_storage::env_secret_store::EnvSecretStore;
    use std::path::PathBuf;
    use tempfile::TempDir;

    struct FakeOcrProvider;

    #[async_trait]
    impl OcrProvider for FakeOcrProvider {
        async fn extract_elements(
            &self,
            image: &[u8],
            image_format: &str,
        ) -> Result<Vec<OcrResult>, CoreError> {
            if image.is_empty() {
                return Ok(vec![]);
            }

            if image_format != "webp" {
                return Err(CoreError::OcrError {
                    code: oneshim_core::error_codes::ProviderCode::OcrFailed,
                    message: format!("예상치 못한 포맷: {image_format}"),
                });
            }

            Ok(vec![OcrResult {
                text: "save".to_string(),
                x: 100,
                y: 100,
                width: 60,
                height: 24,
                confidence: 0.9,
            }])
        }

        fn provider_name(&self) -> &str {
            "fake-ocr"
        }

        fn is_external(&self) -> bool {
            false
        }
    }

    async fn create_test_storage(base_dir: PathBuf) -> FrameFileStorage {
        FrameFileStorage::new(base_dir, 100, 7)
            .await
            .expect("Failed to create test frame storage")
    }

    #[cfg(feature = "server")]
    struct StaticProcessMonitor {
        active_window: Option<WindowInfo>,
    }

    #[cfg(feature = "server")]
    #[async_trait]
    impl ProcessMonitor for StaticProcessMonitor {
        async fn get_active_window(&self) -> Result<Option<WindowInfo>, CoreError> {
            Ok(self.active_window.clone())
        }

        async fn get_top_processes(&self, _limit: usize) -> Result<Vec<ProcessInfo>, CoreError> {
            Ok(vec![])
        }

        async fn get_detailed_processes(
            &self,
            _foreground_pid: Option<u32>,
            _top_n: usize,
        ) -> Result<Vec<ProcessDetail>, CoreError> {
            Ok(vec![])
        }
    }

    #[cfg(feature = "server")]
    fn remote_ocr_guard(temp_dir: &TempDir) -> ExternalOcrPrivacyGuard {
        let consent_path = temp_dir.path().join("consent.json");
        let mut consent_manager = ConsentManager::new(consent_path.clone());
        consent_manager
            .grant_consent(
                ConsentPermissions {
                    ocr_processing: true,
                    screen_capture: true,
                    ..Default::default()
                },
                30,
            )
            .expect("Failed to write consent");

        ExternalOcrPrivacyGuard::new(
            consent_path,
            PiiFilterLevel::Standard,
            oneshim_core::config::ExternalDataPolicy::PiiFilterStandard,
            PrivacyConfig::default(),
            Arc::new(StaticProcessMonitor {
                active_window: Some(WindowInfo {
                    title: "main.rs".to_string(),
                    app_name: "Code".to_string(),
                    pid: 42,
                    bounds: None,
                }),
            }),
            None,
        )
    }

    #[cfg(feature = "server")]
    fn secret_bound_remote_endpoint(profile_id: &str) -> ExternalApiEndpoint {
        let (namespace, key) = provider_api_key_secret_ref("generic", profile_id).unwrap();
        ExternalApiEndpoint {
            endpoint: "https://api.example.com/v1".to_string(),
            api_key: String::new(),
            model: Some("model-test".to_string()),
            timeout_secs: 30,
            provider_type: AiProviderType::Generic,
            surface_id: None,
            credential: Some(CredentialBinding {
                auth_mode: CredentialAuthMode::ApiKey,
                backend_kind: CredentialBackendKind::Env,
                secret_ref: Some(SecretRef {
                    namespace,
                    key: key.to_string(),
                }),
                projection_enabled: false,
            }),
        }
    }

    #[cfg(feature = "server")]
    fn remote_secret_stores() -> SecretStoreSet {
        let mut snapshot = std::collections::HashMap::new();
        for profile_id in ["ocr", "llm"] {
            let (namespace, key) = provider_api_key_secret_ref("generic", profile_id).unwrap();
            snapshot.insert(
                secret_env_var_name(&namespace, key),
                "test-api-key".to_string(),
            );
        }
        let secret_store = Arc::new(EnvSecretStore::from_snapshot(snapshot));
        SecretStoreSet {
            os_secret_store: None,
            file_secret_store: None,
            env_secret_store: Some(secret_store),
            default_backend_kind: CredentialBackendKind::Env,
            fallback_backend_kind: CredentialBackendKind::Unavailable,
        }
    }

    #[tokio::test]
    async fn latest_frame_finder_reads_frame_and_matches_text() {
        let temp_dir = TempDir::new().unwrap();
        let storage = Arc::new(create_test_storage(temp_dir.path().to_path_buf()).await);
        storage
            .save_frame(Utc::now(), b"fake-webp-binary")
            .await
            .unwrap();

        let finder = LatestFrameOcrElementFinder::new(storage, Arc::new(FakeOcrProvider));
        let result = finder
            .find_element(Some("save"), Some("button"), None)
            .await
            .unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].text, "save");
    }

    #[tokio::test]
    async fn latest_frame_finder_returns_not_found_when_no_frame_exists() {
        let temp_dir = TempDir::new().unwrap();
        let storage = Arc::new(create_test_storage(temp_dir.path().to_path_buf()).await);
        let finder = LatestFrameOcrElementFinder::new(storage, Arc::new(FakeOcrProvider));

        let err = finder
            .find_element(Some("save"), None, None)
            .await
            .unwrap_err();
        assert!(matches!(err, CoreError::ElementNotFound { .. }));
    }

    #[test]
    #[cfg(feature = "server")]
    fn build_runtime_falls_back_when_remote_config_is_missing() {
        let config = AiProviderConfig {
            ocr_provider: OcrProviderType::Remote,
            llm_provider: LlmProviderType::Remote,
            ocr_api: None,
            llm_api: None,
            fallback_to_local: true,
            ..AiProviderConfig::default()
        };

        let runtime = build_automation_runtime(
            &config,
            PiiFilterLevel::Standard,
            None,
            None,
            None,
            None,
            #[cfg(feature = "server")]
            None,
        )
        .unwrap();
        assert_eq!(runtime.access_mode, AiAccessMode::ProviderApiKey);
        assert_eq!(runtime.ocr_source, ProviderSource::LocalFallback);
        assert_eq!(runtime.llm_source, ProviderSource::LocalFallback);
        assert!(runtime
            .ocr_fallback_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("ocr_api")));
        assert!(runtime
            .llm_fallback_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("llm_api")));
    }

    #[test]
    fn build_runtime_errors_when_remote_config_missing_and_fallback_disabled() {
        let config = AiProviderConfig {
            ocr_provider: OcrProviderType::Remote,
            llm_provider: LlmProviderType::Local,
            ocr_api: None,
            llm_api: None,
            fallback_to_local: false,
            ..AiProviderConfig::default()
        };

        match build_automation_runtime(
            &config,
            PiiFilterLevel::Standard,
            None,
            None,
            None,
            None,
            #[cfg(feature = "server")]
            None,
        ) {
            Ok(_) => panic!("Expected an error"),
            // Iter-109: emission variant depends on whether the `server`
            // feature is enabled:
            // - with server: CoreError::Config (config missing — iter-99)
            // - without server: CoreError::ServiceUnavailable (feature gate)
            // Accept both since the test runs under different feature
            // combinations.
            Err(err) => assert!(
                matches!(
                    err,
                    CoreError::Config { .. } | CoreError::ServiceUnavailable { .. }
                ),
                "expected Config or ServiceUnavailable, got: {err:?}"
            ),
        }
    }

    #[test]
    #[cfg(feature = "server")]
    fn build_runtime_uses_remote_sources_when_endpoints_are_valid() {
        let ocr_endpoint = secret_bound_remote_endpoint("ocr");
        let llm_endpoint = secret_bound_remote_endpoint("llm");
        let config = AiProviderConfig {
            ocr_provider: OcrProviderType::Remote,
            llm_provider: LlmProviderType::Remote,
            ocr_api: Some(ocr_endpoint),
            llm_api: Some(llm_endpoint),
            fallback_to_local: false,
            ..AiProviderConfig::default()
        };

        let temp_dir = TempDir::new().unwrap();
        let runtime = build_automation_runtime(
            &config,
            PiiFilterLevel::Standard,
            None,
            Some(remote_ocr_guard(&temp_dir)),
            None,
            Some(remote_secret_stores()),
            #[cfg(feature = "server")]
            None,
        )
        .unwrap();
        assert_eq!(runtime.ocr_source, ProviderSource::Remote);
        assert_eq!(runtime.llm_source, ProviderSource::Remote);
        assert!(runtime.ocr_fallback_reason.is_none());
        assert!(runtime.llm_fallback_reason.is_none());
        assert_eq!(runtime.ocr_provider_name, "remote-ocr");
    }

    #[test]
    #[cfg(feature = "server")]
    fn build_runtime_requires_external_ocr_privacy_guard_for_remote_ocr() {
        let ocr_endpoint = secret_bound_remote_endpoint("ocr");
        let config = AiProviderConfig {
            ocr_provider: OcrProviderType::Remote,
            llm_provider: LlmProviderType::Local,
            ocr_api: Some(ocr_endpoint),
            fallback_to_local: false,
            ..AiProviderConfig::default()
        };

        let result = build_automation_runtime(
            &config,
            PiiFilterLevel::Standard,
            None,
            None,
            None,
            Some(remote_secret_stores()),
            #[cfg(feature = "server")]
            None,
        );
        assert!(
            result.is_err(),
            "Remote OCR should require a runtime privacy guard"
        );
        let err = result.err().unwrap();
        assert!(err.to_string().contains("runtime privacy guard"));
    }
}
