#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum ProviderTransportKind {
    Llm,
    Ocr,
    ModelCatalog,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum ProviderAuthScheme {
    None,
    Bearer,
    XApiKey,
    XGoogApiKey,
    AwsSignatureV4,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum ProviderRequestShape {
    AnthropicMessages,
    AnthropicVisionMessages,
    OpenAiChatCompletions,
    OpenAiVisionChatCompletions,
    OpenAiResponses,
    GoogleGenerateContent,
    GoogleVisionAnnotate,
    BedrockConverse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum ModelCatalogResponseShape {
    StandardDataOrModels,
    GoogleModels,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum SurfaceCapabilityKind {
    Llm,
    Ocr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum SurfaceModelCapabilityKind {
    Llm,
    Ocr,
    ImageInput,
    StructuredOutput,
}

impl From<SurfaceCapabilityKind> for SurfaceModelCapabilityKind {
    fn from(value: SurfaceCapabilityKind) -> Self {
        match value {
            SurfaceCapabilityKind::Llm => SurfaceModelCapabilityKind::Llm,
            SurfaceCapabilityKind::Ocr => SurfaceModelCapabilityKind::Ocr,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum SurfaceExecutionKind {
    DirectHttp,
    ManagedHttp,
    SubprocessCli,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum SurfacePlacementKind {
    ProviderHosted,
    SelfHosted,
    InstalledCli,
    CustomHosted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum SurfaceStability {
    Ga,
    Preview,
    Experimental,
    Deprecated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum ModelCatalogStrategy {
    None,
    HttpModelsEndpoint,
    SubprocessProbe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum SubprocessInvocationMode {
    CodexExecJson,
    ClaudePrintJson,
    GeminiCliPrompt,
    ManualChatGui,
    /// Codex `app-server` JSON-RPC 2.0 transport (E21 #4864). The headless
    /// runtime adapter lands in #4865; until then this mode is recognized but
    /// has no runtime (treated like `ManualChatGui` — `runtime_supported=false`).
    CodexAppServer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum SubprocessAuthProbeMode {
    None,
    CodexLoginStatusText,
    ClaudeAuthStatusJson,
    /// Codex `app-server` structured auth status via the read-only `account/read`
    /// JSON-RPC method (E21 #4868 Part 1). This replaces the fragile
    /// `codex login status` stdout text grep for the app-server surface only; the
    /// exec fallback surface keeps [`SubprocessAuthProbeMode::CodexLoginStatusText`].
    ///
    /// `account/read` is READ-ONLY (ADR-025-blessed). It never reads, persists,
    /// relays, or injects the ChatGPT OAuth token; the CLI owns OAuth. The
    /// token-injection path (`account/login/start`, `chatgptAuthTokens`) is
    /// legal-gated (ADR-025 Decision 4 + #4868 B4/R5) and explicitly NOT part of
    /// this mode.
    CodexAccountReadJson,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ProviderUnknownModelPolicy {
    Allow,
    Warn,
    Reject,
}
