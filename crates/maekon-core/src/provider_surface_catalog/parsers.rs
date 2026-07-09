use crate::config::AiProviderType;
use crate::provider_surface::provider_type_from_vendor_id;

use super::enums::{
    ModelCatalogStrategy, ProviderAuthScheme, ProviderRequestShape, SubprocessAuthProbeMode,
    SubprocessInvocationMode, SurfaceExecutionKind, SurfacePlacementKind, SurfaceStability,
};
use super::models::ProviderModelSupportStatus;

pub fn parse_surface_execution_kind(raw: &str) -> Result<SurfaceExecutionKind, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "direct_http" => Ok(SurfaceExecutionKind::DirectHttp),
        "managed_http" => Ok(SurfaceExecutionKind::ManagedHttp),
        "subprocess_cli" => Ok(SurfaceExecutionKind::SubprocessCli),
        other => Err(format!("Unsupported surface execution kind '{other}'.")),
    }
}

pub fn parse_surface_placement_kind(raw: &str) -> Result<SurfacePlacementKind, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "provider_hosted" => Ok(SurfacePlacementKind::ProviderHosted),
        "self_hosted" => Ok(SurfacePlacementKind::SelfHosted),
        "installed_cli" => Ok(SurfacePlacementKind::InstalledCli),
        "custom_hosted" => Ok(SurfacePlacementKind::CustomHosted),
        other => Err(format!(
            "Unsupported provider surface placement_kind '{other}'."
        )),
    }
}

pub fn parse_surface_stability(raw: &str) -> Result<SurfaceStability, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "ga" => Ok(SurfaceStability::Ga),
        "preview" => Ok(SurfaceStability::Preview),
        "experimental" => Ok(SurfaceStability::Experimental),
        "deprecated" => Ok(SurfaceStability::Deprecated),
        other => Err(format!("Unsupported surface stability '{other}'.")),
    }
}

pub fn parse_model_catalog_strategy(raw: &str) -> Result<ModelCatalogStrategy, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "none" => Ok(ModelCatalogStrategy::None),
        "http_models_endpoint" => Ok(ModelCatalogStrategy::HttpModelsEndpoint),
        "subprocess_probe" => Ok(ModelCatalogStrategy::SubprocessProbe),
        other => Err(format!("Unsupported model catalog strategy '{other}'.")),
    }
}

pub fn parse_subprocess_invocation_mode(raw: &str) -> Result<SubprocessInvocationMode, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "codex_exec_json" => Ok(SubprocessInvocationMode::CodexExecJson),
        "claude_print_json" => Ok(SubprocessInvocationMode::ClaudePrintJson),
        "gemini_cli_prompt" => Ok(SubprocessInvocationMode::GeminiCliPrompt),
        "manual_chat_gui" => Ok(SubprocessInvocationMode::ManualChatGui),
        "codex_app_server" => Ok(SubprocessInvocationMode::CodexAppServer),
        other => Err(format!("Unsupported subprocess invocation mode '{other}'.")),
    }
}

pub fn parse_subprocess_auth_probe_mode(raw: &str) -> Result<SubprocessAuthProbeMode, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "none" => Ok(SubprocessAuthProbeMode::None),
        "codex_login_status_text" => Ok(SubprocessAuthProbeMode::CodexLoginStatusText),
        "claude_auth_status_json" => Ok(SubprocessAuthProbeMode::ClaudeAuthStatusJson),
        // E21 #4868 Part 1: structured read-only `account/read` probe for the
        // codex app-server surface (replaces the text grep on that surface only).
        "codex_account_read_json" => Ok(SubprocessAuthProbeMode::CodexAccountReadJson),
        other => Err(format!("Unsupported subprocess auth probe mode '{other}'.")),
    }
}

pub(super) fn parse_auth_scheme(raw: &str) -> Result<ProviderAuthScheme, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "none" => Ok(ProviderAuthScheme::None),
        "bearer" => Ok(ProviderAuthScheme::Bearer),
        "x_api_key" => Ok(ProviderAuthScheme::XApiKey),
        "x_goog_api_key" => Ok(ProviderAuthScheme::XGoogApiKey),
        "aws_signature_v4" => Ok(ProviderAuthScheme::AwsSignatureV4),
        _ => Err(format!("Unsupported provider auth scheme '{raw}'")),
    }
}

pub(super) fn parse_request_shape(raw: &str) -> Result<ProviderRequestShape, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "anthropic_messages" => Ok(ProviderRequestShape::AnthropicMessages),
        "anthropic_vision_messages" => Ok(ProviderRequestShape::AnthropicVisionMessages),
        "openai_chat_completions" => Ok(ProviderRequestShape::OpenAiChatCompletions),
        "openai_vision_chat_completions" => Ok(ProviderRequestShape::OpenAiVisionChatCompletions),
        "openai_responses" => Ok(ProviderRequestShape::OpenAiResponses),
        "google_generate_content" => Ok(ProviderRequestShape::GoogleGenerateContent),
        "google_vision_annotate" => Ok(ProviderRequestShape::GoogleVisionAnnotate),
        "bedrock_converse" => Ok(ProviderRequestShape::BedrockConverse),
        _ => Err(format!("Unsupported provider request shape '{raw}'")),
    }
}

pub(super) fn parse_provider_type_name(raw: &str) -> Option<AiProviderType> {
    provider_type_from_vendor_id(raw)
}

pub(super) fn parse_provider_type(raw: &str) -> Result<AiProviderType, String> {
    parse_provider_type_name(raw).ok_or_else(|| {
        format!(
            "Unsupported provider_type '{}'.",
            raw.trim().to_ascii_lowercase()
        )
    })
}

pub(super) fn default_model_support_status(raw: &str) -> ProviderModelSupportStatus {
    match raw.trim().to_ascii_lowercase().as_str() {
        "supported" => ProviderModelSupportStatus::Supported,
        "unsupported" => ProviderModelSupportStatus::Unsupported,
        _ => ProviderModelSupportStatus::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_codex_app_server_invocation_mode() {
        // E21-2 (#4864): app-server is a new headless invocation mode alongside
        // the exec-based ones. The runtime adapter lands in #4865.
        assert_eq!(
            parse_subprocess_invocation_mode("codex_app_server"),
            Ok(SubprocessInvocationMode::CodexAppServer)
        );
    }

    #[test]
    fn codex_exec_json_invocation_mode_unchanged() {
        // Regression guard: adding the app-server variant must not disturb the
        // existing exec path (#4864 AC).
        assert_eq!(
            parse_subprocess_invocation_mode("codex_exec_json"),
            Ok(SubprocessInvocationMode::CodexExecJson)
        );
    }

    #[test]
    fn unknown_invocation_mode_still_errors() {
        let err = parse_subprocess_invocation_mode("nope_not_a_mode").unwrap_err();
        assert!(
            err.contains("Unsupported subprocess invocation mode"),
            "unknown mode must report unsupported; got: {err}"
        );
    }

    #[test]
    fn parses_codex_account_read_auth_probe_mode() {
        // E21 #4868 Part 1: the new structured account/read probe mode string maps
        // to the dedicated variant.
        assert_eq!(
            parse_subprocess_auth_probe_mode("codex_account_read_json"),
            Ok(SubprocessAuthProbeMode::CodexAccountReadJson)
        );
    }

    #[test]
    fn existing_auth_probe_modes_unchanged() {
        // Regression guard: adding the account/read variant must not disturb the
        // existing text/JSON probe modes.
        assert_eq!(
            parse_subprocess_auth_probe_mode("codex_login_status_text"),
            Ok(SubprocessAuthProbeMode::CodexLoginStatusText)
        );
        assert_eq!(
            parse_subprocess_auth_probe_mode("claude_auth_status_json"),
            Ok(SubprocessAuthProbeMode::ClaudeAuthStatusJson)
        );
        assert_eq!(
            parse_subprocess_auth_probe_mode("none"),
            Ok(SubprocessAuthProbeMode::None)
        );
    }

    #[test]
    fn unknown_auth_probe_mode_still_errors() {
        let err = parse_subprocess_auth_probe_mode("nope_not_a_probe_mode").unwrap_err();
        assert!(
            err.contains("Unsupported subprocess auth probe mode"),
            "unknown probe mode must report unsupported; got: {err}"
        );
    }
}
