use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PiiFilterLevel {
    Off,
    Basic,
    #[default]
    Standard,
    Strict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Weekday {
    Mon,
    Tue,
    Wed,
    Thu,
    Fri,
    Sat,
    Sun,
}

impl Weekday {
    /// Number of days from Sunday (compatible with `chrono::Weekday::num_days_from_sunday()`).
    pub fn num_days_from_sunday(self) -> u32 {
        match self {
            Weekday::Sun => 0,
            Weekday::Mon => 1,
            Weekday::Tue => 2,
            Weekday::Wed => 3,
            Weekday::Thu => 4,
            Weekday::Fri => 5,
            Weekday::Sat => 6,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SandboxProfile {
    Permissive,
    #[default]
    Standard,
    Strict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum OcrProviderType {
    #[default]
    Local,
    Remote,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum LlmProviderType {
    #[default]
    Local,
    Remote,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AiAccessMode {
    #[default]
    ProviderApiKey,
    LocalModel,
    ProviderSubscriptionCli,
    ProviderOAuth,
}

impl AiAccessMode {
    pub fn normalized_for_ai_surfaces(self) -> Self {
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AiProviderType {
    Anthropic,
    OpenAi,
    Google,
    Ollama,
    Bedrock,
    Copilot,
    #[default]
    Generic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ExternalDataPolicy {
    #[default]
    PiiFilterStrict,
    PiiFilterStandard,
    AllowFiltered,
}

/// Message tone style for coaching messages.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CoachingTone {
    /// Short, actionable statements.
    Direct,
    /// Softer, encouraging language.
    #[default]
    Gentle,
    /// Statistics-focused with numbers and comparisons.
    DataDriven,
}

/// Historical comparison window for coaching baselines.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataLookback {
    /// Compare against today's data only.
    #[default]
    Today,
    /// Rolling 7-day comparison.
    Week,
    /// Rolling 30-day comparison.
    Month,
}

/// Overlay display mode (Phase 2 — MagicOverlay). Stored in config for forward compatibility.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum OverlayMode {
    /// Focus area border + coaching popup only.
    #[default]
    Minimal,
    /// + bottom progress bar + attention heatmap ghost.
    Rich,
    /// Auto-switches based on regime (deep work -> Minimal, transition -> Rich).
    Adaptive,
}

/// Speech-to-text language hint for Whisper transcription.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SttLanguage {
    #[default]
    Auto,
    En,
    Ko,
}

/// Available Whisper model variants for local STT.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WhisperModelSize {
    Tiny,
    #[default]
    Base,
    Small,
    Medium,
}

impl WhisperModelSize {
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Tiny => "Tiny (~75 MB)",
            Self::Base => "Base (~142 MB)",
            Self::Small => "Small (~466 MB)",
            Self::Medium => "Medium (~1.5 GB)",
        }
    }
}

/// STT provider selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SttProviderKind {
    #[default]
    Local,
    Cloud,
}

/// Mic input mode selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MicInputMode {
    #[default]
    PushToTalk,
    VoiceActivity,
}

impl std::fmt::Display for MicInputMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PushToTalk => write!(f, "push_to_talk"),
            Self::VoiceActivity => write!(f, "voice_activity"),
        }
    }
}

/// Confirmation requirement for automation command execution.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConfirmationRequirement {
    /// Low-risk: execute without user prompt
    Auto,
    /// Medium-risk: show confirmation modal
    #[default]
    Confirm,
    /// High-risk: block execution entirely
    Block,
}
