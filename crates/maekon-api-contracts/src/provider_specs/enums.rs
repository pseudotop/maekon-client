#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderTransportKind {
    Llm,
    Ocr,
    ModelCatalog,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderAuthScheme {
    None,
    Bearer,
    XApiKey,
    XGoogApiKey,
    AwsSignatureV4,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
pub enum ModelCatalogResponseShape {
    StandardDataOrModels,
    GoogleModels,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceCapabilityKind {
    Llm,
    Ocr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
pub enum SurfaceExecutionKind {
    DirectHttp,
    ManagedHttp,
    SubprocessCli,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfacePlacementKind {
    ProviderHosted,
    SelfHosted,
    InstalledCli,
    CustomHosted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceStability {
    Ga,
    Preview,
    Experimental,
    Deprecated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelCatalogStrategy {
    None,
    HttpModelsEndpoint,
    SubprocessProbe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubprocessInvocationMode {
    CodexExecJson,
    ClaudePrintJson,
    GeminiCliPrompt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubprocessAuthProbeMode {
    None,
    CodexLoginStatusText,
    ClaudeAuthStatusJson,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderUnknownModelPolicy {
    Allow,
    Warn,
    Reject,
}
