// AI provider config — AiProviderConfig and external-endpoint validation orchestrator
use super::super::enums::*;
use super::ai_validation::{
    CredentialAuthMode, ExternalApiEndpoint, OcrValidationConfig, SceneActionOverrideConfig,
    SceneIntelligenceConfig,
};
use crate::error::CoreError;
use crate::provider_surface::{
    provider_surface_spec, provider_surface_supports_llm, provider_surface_supports_ocr,
    provider_surface_uses_no_auth, ProviderSurfaceTransport,
};
use serde::{Deserialize, Serialize};

// ── AiProviderConfig ───────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiProviderConfig {
    #[serde(default)]
    pub access_mode: AiAccessMode,
    #[serde(default)]
    pub ocr_provider: OcrProviderType,
    #[serde(default)]
    pub llm_provider: LlmProviderType,
    #[serde(default)]
    pub ocr_api: Option<ExternalApiEndpoint>,
    #[serde(default)]
    pub llm_api: Option<ExternalApiEndpoint>,
    #[serde(default)]
    pub llm_api_fallback: Option<ExternalApiEndpoint>,
    #[serde(default)]
    pub external_data_policy: ExternalDataPolicy,
    // #5966: renamed from `allow_unredacted_external_ocr` for clarity. The serde
    // alias keeps existing on-disk config.json files deserializing. Activation of
    // the bypass additionally requires the explicit `unredacted_external_ocr`
    // consent tier (enforced in `PrivacyGateway::prepare_image_for_external_with_override`).
    #[serde(default, alias = "allow_unredacted_external_ocr")]
    pub bypass_pii_filter_for_external_ocr: bool,
    #[serde(default)]
    pub ocr_validation: OcrValidationConfig,
    #[serde(default)]
    pub scene_action_override: SceneActionOverrideConfig,
    #[serde(default)]
    pub scene_intelligence: SceneIntelligenceConfig,
    #[serde(default = "default_true")]
    pub fallback_to_local: bool,
    /// Rollout stage for the Codex `app-server` JSON-RPC transport (E21 #4871).
    /// `Off` (default) keeps the `codex exec` path; opt-in/default attempt
    /// app-server with graceful fallback to exec on failure.
    #[serde(default)]
    pub codex_app_server_rollout: CodexAppServerRollout,
    #[serde(default)]
    pub active_profile_id: Option<String>,
    #[serde(default)]
    pub saved_profiles: Vec<SavedAiProviderProfile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiProviderProfileConfig {
    #[serde(default)]
    pub access_mode: AiAccessMode,
    #[serde(default)]
    pub ocr_provider: OcrProviderType,
    #[serde(default)]
    pub llm_provider: LlmProviderType,
    #[serde(default)]
    pub ocr_api: Option<ExternalApiEndpoint>,
    #[serde(default)]
    pub llm_api: Option<ExternalApiEndpoint>,
    #[serde(default)]
    pub external_data_policy: ExternalDataPolicy,
    // #5966: renamed from `allow_unredacted_external_ocr` (see `AiProviderConfig`).
    #[serde(default, alias = "allow_unredacted_external_ocr")]
    pub bypass_pii_filter_for_external_ocr: bool,
    #[serde(default)]
    pub ocr_validation: OcrValidationConfig,
    #[serde(default)]
    pub scene_action_override: SceneActionOverrideConfig,
    #[serde(default)]
    pub scene_intelligence: SceneIntelligenceConfig,
    #[serde(default = "default_true")]
    pub fallback_to_local: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedAiProviderProfile {
    pub profile_id: String,
    pub name: String,
    #[serde(default)]
    pub ai_provider: AiProviderProfileConfig,
    #[serde(default)]
    pub updated_at: Option<String>,
}

impl Default for AiProviderConfig {
    fn default() -> Self {
        Self {
            access_mode: AiAccessMode::default(),
            ocr_provider: OcrProviderType::default(),
            llm_provider: LlmProviderType::default(),
            ocr_api: None,
            llm_api: None,
            llm_api_fallback: None,
            external_data_policy: ExternalDataPolicy::default(),
            bypass_pii_filter_for_external_ocr: false,
            ocr_validation: OcrValidationConfig::default(),
            scene_action_override: SceneActionOverrideConfig::default(),
            scene_intelligence: SceneIntelligenceConfig::default(),
            fallback_to_local: true,
            codex_app_server_rollout: CodexAppServerRollout::default(),
            active_profile_id: None,
            saved_profiles: Vec::new(),
        }
    }
}

impl Default for AiProviderProfileConfig {
    fn default() -> Self {
        Self {
            access_mode: AiAccessMode::default(),
            ocr_provider: OcrProviderType::default(),
            llm_provider: LlmProviderType::default(),
            ocr_api: None,
            llm_api: None,
            external_data_policy: ExternalDataPolicy::default(),
            bypass_pii_filter_for_external_ocr: false,
            ocr_validation: OcrValidationConfig::default(),
            scene_action_override: SceneActionOverrideConfig::default(),
            scene_intelligence: SceneIntelligenceConfig::default(),
            fallback_to_local: true,
        }
    }
}

impl Default for SavedAiProviderProfile {
    fn default() -> Self {
        Self {
            profile_id: "ai-profile".to_string(),
            name: "AI Profile".to_string(),
            ai_provider: AiProviderProfileConfig::default(),
            updated_at: None,
        }
    }
}

impl AiProviderConfig {
    pub fn active_secret_profile_id_or<'a>(&'a self, fallback_profile_id: &'a str) -> &'a str {
        self.active_profile_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(fallback_profile_id)
    }

    pub fn validate_selected_remote_endpoints(&self) -> Result<(), CoreError> {
        self.ocr_validation.validate()?;
        self.scene_action_override.validate()?;
        self.scene_intelligence.validate()?;

        match self.access_mode.normalized_for_ai_surfaces() {
            AiAccessMode::ProviderApiKey => {
                if self.ocr_provider == OcrProviderType::Remote {
                    validate_remote_endpoint(self.ocr_api.as_ref(), "ocr_api")?;
                }
                if self.llm_provider == LlmProviderType::Remote {
                    validate_remote_endpoint(self.llm_api.as_ref(), "llm_api")?;
                }
            }
            AiAccessMode::LocalModel => {
                if self
                    .llm_api
                    .as_ref()
                    .is_some_and(|endpoint| endpoint.provider_type == AiProviderType::Ollama)
                {
                    validate_local_model_ollama_endpoint(self.llm_api.as_ref(), "llm_api")?;
                }
            }
            AiAccessMode::ProviderSubscriptionCli => {
                if self.ocr_provider == OcrProviderType::Remote {
                    validate_subprocess_or_remote_endpoint(
                        self.ocr_api.as_ref(),
                        "ocr_api",
                        ProviderSurfaceTransport::SubprocessCli,
                        provider_surface_supports_ocr,
                    )?;
                }
                if self.llm_provider == LlmProviderType::Remote {
                    validate_non_http_surface_endpoint(
                        self.llm_api.as_ref(),
                        "llm_api",
                        ProviderSurfaceTransport::SubprocessCli,
                        provider_surface_supports_llm,
                    )?;
                }
            }
            AiAccessMode::ProviderOAuth => {
                if self.llm_provider == LlmProviderType::Remote {
                    validate_managed_or_remote_endpoint(
                        self.llm_api.as_ref(),
                        "llm_api",
                        ProviderSurfaceTransport::ManagedOAuth,
                        provider_surface_supports_llm,
                    )?;
                }
                if self.ocr_provider == OcrProviderType::Remote {
                    validate_managed_or_remote_endpoint(
                        self.ocr_api.as_ref(),
                        "ocr_api",
                        ProviderSurfaceTransport::ManagedOAuth,
                        provider_surface_supports_ocr,
                    )?;
                }
            }
        }
        Ok(())
    }
}

// ── validate_remote_endpoint (module-internal helpers) ────────────

/// #6259: std-only "confidently-remote IP" classifier for the config-load
/// cleartext gate.
///
/// maekon-core must not depend on maekon-network/url, so a perfect parity with
/// the authoritative url-based runtime gate
/// (`provider_adapters::require_endpoint_config` →
/// `maekon_network::http_client::host_is_loopback`) is not achievable here.
/// To avoid FALSE-REJECTING a genuinely-loopback endpoint (e.g. exotic-but-valid
/// encodings `http://127.1`, decimal/octal/hex IPs) at config-load, this check
/// only reports `true` when the host parses as a NON-loopback IP literal — i.e.
/// a host we are confident is remote. Anything we cannot classify in std (DNS
/// names, alternate IP encodings, userinfo tricks) returns `false` and is
/// deferred to the authoritative runtime gate, which makes the real decision.
/// This keeps any divergence strictly in the safe direction (config-load never
/// blocks a valid loopback config, and never wrongly waves through remote
/// cleartext — the runtime gate is the fail-closed boundary).
fn endpoint_is_confident_remote_ip(url: &str) -> bool {
    // Strip scheme (`http://` / `https://`).
    let Some((_, rest)) = url.split_once("://") else {
        return false;
    };
    // Authority ends at the first '/', '?', or '#'.
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    // Drop any `userinfo@` prefix (take the part after the last '@').
    let host_port = authority
        .rsplit_once('@')
        .map(|(_, hp)| hp)
        .unwrap_or(authority);
    // Extract host, handling `[ipv6]:port`, `host:port`, and bare host.
    let host = if let Some(after_bracket) = host_port.strip_prefix('[') {
        match after_bracket.split_once(']') {
            Some((h, _)) => h,
            None => return false, // malformed bracket form → defer to runtime
        }
    } else {
        host_port.split(':').next().unwrap_or(host_port)
    };
    // Only a parseable, non-loopback IP literal is "confidently remote". A
    // loopback IP, `localhost`, a DNS name, or any form std cannot parse is NOT
    // confidently remote (→ false), so it is never false-rejected here.
    match host.trim().parse::<std::net::IpAddr>() {
        Ok(addr) => !addr.is_loopback(),
        Err(_) => false,
    }
}

fn validate_remote_endpoint(
    endpoint: Option<&ExternalApiEndpoint>,
    field_name: &str,
) -> Result<(), CoreError> {
    let endpoint = endpoint.ok_or_else(|| CoreError::Config {
        code: crate::error_codes::ConfigCode::Missing,
        message: format!("`{field_name}` is required when a remote provider is selected."),
    })?;

    let endpoint_url = endpoint.endpoint.trim();
    if endpoint_url.is_empty() {
        return Err(CoreError::Config {
            code: crate::error_codes::ConfigCode::Missing,
            message: format!("`{field_name}.endpoint` must not be empty."),
        });
    }
    if !(endpoint_url.starts_with("http://") || endpoint_url.starts_with("https://")) {
        return Err(CoreError::Config {
            code: crate::error_codes::ConfigCode::Invalid,
            message: format!("`{field_name}.endpoint` must be an http:// or https:// URL."),
        });
    }
    // #6259: reject remote cleartext at config-load time (early feedback ahead of
    // the authoritative runtime gate `provider_adapters::require_endpoint_config`).
    // A remote `http://` endpoint would egress the Bearer API key + screen context
    // in cleartext. We only reject when the host is CONFIDENTLY a remote IP literal
    // (parseable, non-loopback) so this std-only check never false-rejects a valid
    // loopback config (`http://localhost`, `http://127.0.0.1`, and exotic-but-valid
    // forms are deferred to the url-based runtime gate, which is the fail-closed
    // boundary). `https://` is always allowed.
    if endpoint_url.starts_with("http://") && endpoint_is_confident_remote_ip(endpoint_url) {
        return Err(CoreError::Config {
            code: crate::error_codes::ConfigCode::Invalid,
            message: format!(
                "`{field_name}.endpoint` uses cleartext http:// to a non-loopback host; \
                 use https:// for a remote endpoint (cleartext is allowed only for \
                 loopback/local providers) to avoid leaking the API key and screen context."
            ),
        });
    }

    let has_api_key_binding = endpoint.credential.as_ref().is_some_and(|binding| {
        binding.auth_mode == CredentialAuthMode::ApiKey && binding.secret_ref.is_some()
    });

    let requires_plaintext_api_key = endpoint
        .surface_id
        .as_deref()
        .map(|surface_id| !provider_surface_uses_no_auth(surface_id))
        .unwrap_or(true);

    if requires_plaintext_api_key && endpoint.api_key.trim().is_empty() && !has_api_key_binding {
        return Err(CoreError::Config {
            code: crate::error_codes::ConfigCode::Missing,
            message: format!(
            "`{field_name}.api_key` must not be empty unless a credential binding is configured."
        ),
        });
    }

    if endpoint.timeout_secs == 0 {
        return Err(CoreError::Config {
            code: crate::error_codes::ConfigCode::OutOfRange,
            message: format!("`{field_name}.timeout_secs` must be >= 1."),
        });
    }

    reject_blocked_model(endpoint)?;

    Ok(())
}

fn validate_local_model_ollama_endpoint(
    endpoint: Option<&ExternalApiEndpoint>,
    field_name: &str,
) -> Result<(), CoreError> {
    let Some(endpoint) = endpoint else {
        return Ok(());
    };

    let endpoint_url = endpoint.endpoint.trim();
    if endpoint_url.is_empty() {
        return Err(CoreError::Config {
            code: crate::error_codes::ConfigCode::Missing,
            message: format!("`{field_name}.endpoint` must not be empty."),
        });
    }
    if !(endpoint_url.starts_with("http://") || endpoint_url.starts_with("https://")) {
        return Err(CoreError::Config {
            code: crate::error_codes::ConfigCode::Invalid,
            message: format!("`{field_name}.endpoint` must be an http:// or https:// URL."),
        });
    }
    if endpoint_url.starts_with("http://") && endpoint_is_confident_remote_ip(endpoint_url) {
        return Err(CoreError::Config {
            code: crate::error_codes::ConfigCode::Invalid,
            message: format!(
                "`{field_name}.endpoint` uses cleartext http:// to a non-loopback host; \
                 use https:// for a remote Ollama endpoint (cleartext is allowed only for \
                 loopback/local Ollama) to avoid leaking screen context."
            ),
        });
    }
    if endpoint.timeout_secs == 0 {
        return Err(CoreError::Config {
            code: crate::error_codes::ConfigCode::OutOfRange,
            message: format!("`{field_name}.timeout_secs` must be >= 1."),
        });
    }

    reject_blocked_model(endpoint)?;
    Ok(())
}

/// Rejects an endpoint whose configured model is past its lifecycle `block_at`.
///
/// This is the single enforcement point for the model-lifecycle `Block`
/// decision at config-validation time. It must be called from *every* surface
/// validation path (remote HTTP, managed OAuth, subprocess CLI) so a retired
/// model cannot be configured via any surface — not just the remote HTTP one.
/// An empty/whitespace model is treated as "no model selected" and allowed.
fn reject_blocked_model(endpoint: &ExternalApiEndpoint) -> Result<(), CoreError> {
    let Some(model) = endpoint
        .model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(());
    };

    let decision = crate::ai_model_lifecycle_policy::evaluate_model_lifecycle_now_for_surface(
        endpoint.provider_type,
        endpoint.surface_id.as_deref(),
        model,
    )?;
    if let crate::ai_model_lifecycle_policy::ModelLifecycleDecision::Block { message, .. } =
        decision
    {
        return Err(CoreError::PolicyDenied {
            code: crate::error_codes::PolicyCode::Denied,
            message,
        });
    }

    Ok(())
}

fn validate_non_http_surface_endpoint(
    endpoint: Option<&ExternalApiEndpoint>,
    field_name: &str,
    expected_transport: ProviderSurfaceTransport,
    capability_check: fn(&str) -> bool,
) -> Result<(), CoreError> {
    let endpoint = endpoint.ok_or_else(|| CoreError::Config {
        code: crate::error_codes::ConfigCode::Missing,
        message: format!("`{field_name}` is required when a managed provider surface is selected."),
    })?;

    let surface_id = endpoint
        .surface_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| CoreError::Config {
            code: crate::error_codes::ConfigCode::Missing,
            message: format!(
                "`{field_name}.surface_id` is required for managed provider surfaces."
            ),
        })?;

    let spec = provider_surface_spec(surface_id).ok_or_else(|| CoreError::Config {
        code: crate::error_codes::ConfigCode::Invalid,
        message: format!("`{field_name}.surface_id` references an unknown provider surface."),
    })?;

    if spec.provider_type != endpoint.provider_type {
        return Err(CoreError::Config {
            code: crate::error_codes::ConfigCode::Invalid,
            message: format!("`{field_name}.surface_id` must match `{field_name}.provider_type`."),
        });
    }

    if spec.transport != expected_transport {
        return Err(CoreError::Config {
            code: crate::error_codes::ConfigCode::Invalid,
            message: format!(
                "`{field_name}.surface_id` is incompatible with the selected access mode."
            ),
        });
    }

    if !capability_check(surface_id) {
        return Err(CoreError::Config {
            code: crate::error_codes::ConfigCode::Invalid,
            message: format!(
                "`{field_name}.surface_id` does not support this endpoint capability."
            ),
        });
    }

    // Managed OAuth / subprocess-CLI surfaces must enforce the model-lifecycle
    // Block decision too — otherwise a retired model could be configured via
    // these surfaces while `validate_remote_endpoint` only guards the HTTP path.
    reject_blocked_model(endpoint)?;

    Ok(())
}

fn validate_subprocess_or_remote_endpoint(
    endpoint: Option<&ExternalApiEndpoint>,
    field_name: &str,
    managed_transport: ProviderSurfaceTransport,
    capability_check: fn(&str) -> bool,
) -> Result<(), CoreError> {
    let Some(endpoint) = endpoint else {
        return Err(CoreError::Config {
            code: crate::error_codes::ConfigCode::Missing,
            message: format!("`{field_name}` is required when a remote provider is selected."),
        });
    };

    if let Some(surface_id) = endpoint
        .surface_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if let Some(surface) = provider_surface_spec(surface_id) {
            if surface.transport == managed_transport {
                return validate_non_http_surface_endpoint(
                    Some(endpoint),
                    field_name,
                    managed_transport,
                    capability_check,
                );
            }
        }
    }

    validate_remote_endpoint(Some(endpoint), field_name)
}

fn validate_managed_or_remote_endpoint(
    endpoint: Option<&ExternalApiEndpoint>,
    field_name: &str,
    managed_transport: ProviderSurfaceTransport,
    capability_check: fn(&str) -> bool,
) -> Result<(), CoreError> {
    let Some(endpoint) = endpoint else {
        return Err(CoreError::Config {
            code: crate::error_codes::ConfigCode::Missing,
            message: format!("`{field_name}` is required when a remote provider is selected."),
        });
    };

    if let Some(surface_id) = endpoint
        .surface_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if let Some(surface) = provider_surface_spec(surface_id) {
            if surface.transport == managed_transport {
                return validate_non_http_surface_endpoint(
                    Some(endpoint),
                    field_name,
                    managed_transport,
                    capability_check,
                );
            }
        }
    }

    validate_remote_endpoint(Some(endpoint), field_name)
}

// ── Private default helpers ─────────────────────────────────────────

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod cleartext_gate_tests {
    use super::*;

    #[test]
    fn endpoint_is_confident_remote_ip_table() {
        // Confidently remote (parseable non-loopback IP) → rejected at config-load.
        assert!(endpoint_is_confident_remote_ip("http://10.0.0.5:8080"));
        assert!(endpoint_is_confident_remote_ip("http://192.168.1.10/api"));
        assert!(endpoint_is_confident_remote_ip(
            "http://203.0.113.7/path?q=1"
        ));
        assert!(endpoint_is_confident_remote_ip("http://[2001:db8::1]:443"));
        // NOT confidently remote (loopback IP / localhost) → never false-rejected.
        assert!(!endpoint_is_confident_remote_ip(
            "http://localhost:11434/v1"
        ));
        assert!(!endpoint_is_confident_remote_ip("http://LocalHost/api"));
        assert!(!endpoint_is_confident_remote_ip("http://127.0.0.1:8080"));
        assert!(!endpoint_is_confident_remote_ip("http://127.5.6.7/path"));
        assert!(!endpoint_is_confident_remote_ip("http://[::1]:9000/v1"));
        assert!(!endpoint_is_confident_remote_ip(
            "http://user:pass@127.0.0.1:1234"
        ));
        // DNS names + forms std cannot classify → deferred to the runtime gate
        // (NOT confidently remote here, so never false-rejected at config-load).
        assert!(!endpoint_is_confident_remote_ip("http://api.openai.com/v1"));
        assert!(!endpoint_is_confident_remote_ip("http://127.1")); // exotic-but-valid loopback
        assert!(!endpoint_is_confident_remote_ip("not-a-url"));
        assert!(!endpoint_is_confident_remote_ip("http://"));
    }

    fn remote_endpoint(url: &str) -> ExternalApiEndpoint {
        ExternalApiEndpoint {
            endpoint: url.to_string(),
            api_key: "sk-test".to_string(),
            model: Some("m".to_string()),
            timeout_secs: 30,
            provider_type: AiProviderType::OpenAi,
            surface_id: None,
            credential: None,
        }
    }

    #[test]
    fn validate_remote_endpoint_rejects_remote_cleartext_ip() {
        // A confidently-remote IP literal over cleartext is rejected at config-load.
        let ep = remote_endpoint("http://10.0.0.5:8080/v1");
        let err = validate_remote_endpoint(Some(&ep), "llm_api")
            .expect_err("remote http:// IP must be rejected as cleartext");
        assert!(format!("{err}").contains("cleartext http://"), "got: {err}");
    }

    #[test]
    fn validate_remote_endpoint_defers_remote_cleartext_dns_to_runtime() {
        // #6259: a DNS host cannot be classified in std-only core, so config-load
        // does NOT reject it (the url-based runtime gate is the fail-closed
        // boundary). Documents the intentional safe-direction divergence:
        // config-load never false-rejects; the runtime gate makes the real call.
        let ep = remote_endpoint("http://api.openai.com/v1");
        validate_remote_endpoint(Some(&ep), "llm_api")
            .expect("DNS cleartext is deferred to the runtime gate, not rejected at config-load");
    }

    #[test]
    fn validate_remote_endpoint_allows_loopback_cleartext() {
        let ep = remote_endpoint("http://127.0.0.1:11434/v1");
        validate_remote_endpoint(Some(&ep), "llm_api")
            .expect("loopback http:// (local provider) must be allowed");
    }

    #[test]
    fn validate_remote_endpoint_allows_remote_https() {
        let ep = remote_endpoint("https://api.openai.com/v1");
        validate_remote_endpoint(Some(&ep), "llm_api").expect("remote https:// must be allowed");
    }

    #[test]
    fn local_model_ollama_rejects_remote_cleartext_ip() {
        let config = AiProviderConfig {
            access_mode: AiAccessMode::LocalModel,
            llm_api: Some(ExternalApiEndpoint {
                endpoint: "http://10.0.0.5:11434/v1/responses".to_string(),
                api_key: String::new(),
                model: Some("qwen3:8b".to_string()),
                timeout_secs: 30,
                provider_type: AiProviderType::Ollama,
                surface_id: Some("provider_surface.ollama.local_http".to_string()),
                credential: None,
            }),
            ..AiProviderConfig::default()
        };

        let err = config
            .validate_selected_remote_endpoints()
            .expect_err("LocalModel Ollama must reject remote cleartext IP endpoints");
        assert!(format!("{err}").contains("cleartext http://"), "got: {err}");
    }
}

#[cfg(test)]
mod bypass_pii_filter_alias_tests {
    use super::*;

    /// #5966: existing on-disk config.json files use the legacy
    /// `allow_unredacted_external_ocr` key. The serde alias must keep those
    /// deserializing (true must survive) so a rename never silently drops a
    /// user's previously-saved bypass setting.
    #[test]
    fn ai_provider_config_deserializes_legacy_allow_unredacted_external_ocr_alias() {
        let legacy_json = r#"{
            "allow_unredacted_external_ocr": true
        }"#;
        let config: AiProviderConfig = serde_json::from_str(legacy_json).unwrap();
        assert!(
            config.bypass_pii_filter_for_external_ocr,
            "legacy `allow_unredacted_external_ocr` must deserialize into the renamed field"
        );
    }

    /// The renamed (canonical) key must also deserialize.
    #[test]
    fn ai_provider_config_deserializes_renamed_key() {
        let new_json = r#"{
            "bypass_pii_filter_for_external_ocr": true
        }"#;
        let config: AiProviderConfig = serde_json::from_str(new_json).unwrap();
        assert!(config.bypass_pii_filter_for_external_ocr);
    }

    /// A missing key must default to false (fail-closed) — the bypass is never
    /// silently enabled.
    #[test]
    fn ai_provider_config_missing_key_defaults_to_false() {
        let config: AiProviderConfig = serde_json::from_str("{}").unwrap();
        assert!(
            !config.bypass_pii_filter_for_external_ocr,
            "missing key must default to false (fail-closed)"
        );
    }

    /// Serialization uses the canonical (renamed) key so newly-written config
    /// files carry the clearer name.
    #[test]
    fn ai_provider_config_serializes_with_renamed_key() {
        let config = AiProviderConfig {
            bypass_pii_filter_for_external_ocr: true,
            ..AiProviderConfig::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        assert!(
            json.contains("bypass_pii_filter_for_external_ocr"),
            "serialized config must use the renamed key, got: {json}"
        );
    }

    /// The profile config mirror carries the same alias contract.
    #[test]
    fn ai_provider_profile_config_deserializes_legacy_alias() {
        let legacy_json = r#"{
            "allow_unredacted_external_ocr": true
        }"#;
        let profile: AiProviderProfileConfig = serde_json::from_str(legacy_json).unwrap();
        assert!(profile.bypass_pii_filter_for_external_ocr);
    }
}
