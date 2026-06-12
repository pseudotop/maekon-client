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
#[serde(rename_all = "PascalCase")]
pub enum SandboxProfile {
    Permissive,
    #[default]
    Standard,
    Strict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub enum OcrProviderType {
    #[default]
    Local,
    Remote,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
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
// 단일 단어 variant 이므로 lowercase → snake_case 동일 결과 (일관성 통일, F-RC-07)
#[serde(rename_all = "snake_case")]
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
#[serde(rename_all = "PascalCase")]
pub enum ExternalDataPolicy {
    #[default]
    PiiFilterStrict,
    PiiFilterStandard,
    AllowFiltered,
}

/// Rollout stage for the Codex `app-server` JSON-RPC transport (E21 #4871).
///
/// The app-server transport is preview/experimental (R1) and pinned to the
/// installed CLI version (R4), so it ships behind a 2-step rollout flag
/// (ADR-070 pattern). The `codex exec` one-shot path is always retained as a
/// graceful fallback; `Off` (the default) preserves the original exec behavior
/// with zero regression.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CodexAppServerRollout {
    /// Never use app-server — always `codex exec` (default, conservative).
    #[default]
    Off,
    /// Opt-in: attempt app-server, fall back to `codex exec` on failure.
    OptIn,
    /// Default-on: attempt app-server, fall back to `codex exec` on failure.
    Default,
}

/// Message tone style for coaching messages.
// F-RC-C37-03: explicit PascalCase wire contract — Display also returns PascalCase.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
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

/// Enterprise managed-policy gate for cloud speech-to-text (STT) egress.
///
/// Cloud STT is a user-directed egress (`audio.stt_provider == Cloud` + a
/// user-supplied `audio.cloud_api_key` + a user-configurable
/// `audio.cloud_stt_endpoint`). This policy lets an IT admin enforce — via a
/// deployed managed config, independent of whether an individual user enters an
/// API key — whether raw audio may leave the device to a third-party endpoint.
///
/// Default is [`CloudSttPolicy::Allow`] to preserve existing (consumer) behavior.
/// Non-`Allow` values are **fail-safe**: cloud egress is blocked until the
/// policy permits it (see [`CloudSttPolicy::permits_cloud_egress`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CloudSttPolicy {
    /// Cloud STT permitted when the user configures it (current default behavior).
    #[default]
    Allow,
    /// Cloud STT requires explicit admin approval before raw audio may egress.
    /// Until an admin-approval channel exists, this blocks egress (fail-safe).
    RequireAdminApproval,
    /// Cloud STT hard-disabled by managed policy — raw audio never leaves the device.
    Disabled,
}

impl CloudSttPolicy {
    /// Returns true only when the managed policy currently permits raw audio to
    /// egress to a cloud STT endpoint. `RequireAdminApproval` returns false until
    /// an approval channel is implemented (fail-safe: never egress unapproved).
    pub fn permits_cloud_egress(self) -> bool {
        matches!(self, CloudSttPolicy::Allow)
    }
}

/// Decision for whether to construct the cloud STT provider at a wiring site.
///
/// Centralizes the egress gate so both the startup and live-reload wiring paths
/// make the identical, unit-tested decision (instead of duplicating the policy +
/// API-key checks inline). See [`super::sections::AudioConfig::cloud_stt_build_decision`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudSttBuild {
    /// Construct the cloud STT provider (policy permits + API key present).
    Build,
    /// Do not construct — managed policy blocks cloud egress (fail-safe).
    SkipPolicyBlocked,
    /// Do not construct — no cloud API key configured by the user.
    SkipNoKey,
}

impl std::fmt::Display for CloudSttPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Allow => f.write_str("allow"),
            Self::RequireAdminApproval => f.write_str("require_admin_approval"),
            Self::Disabled => f.write_str("disabled"),
        }
    }
}

/// Mic input mode selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MicInputMode {
    #[default]
    PushToTalk,
    VoiceActivity,
}

impl std::fmt::Display for PiiFilterLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Off => f.write_str("off"),
            Self::Basic => f.write_str("basic"),
            Self::Standard => f.write_str("standard"),
            Self::Strict => f.write_str("strict"),
        }
    }
}

impl std::fmt::Display for Weekday {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Mon => f.write_str("Mon"),
            Self::Tue => f.write_str("Tue"),
            Self::Wed => f.write_str("Wed"),
            Self::Thu => f.write_str("Thu"),
            Self::Fri => f.write_str("Fri"),
            Self::Sat => f.write_str("Sat"),
            Self::Sun => f.write_str("Sun"),
        }
    }
}

impl std::fmt::Display for SandboxProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // PascalCase — aligned with serde(rename_all = "PascalCase") and dto_to_policy matchers.
        match self {
            Self::Permissive => f.write_str("Permissive"),
            Self::Standard => f.write_str("Standard"),
            Self::Strict => f.write_str("Strict"),
        }
    }
}

impl std::fmt::Display for OcrProviderType {
    /// Returns PascalCase token matching the serde wire representation.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Local => f.write_str("Local"),
            Self::Remote => f.write_str("Remote"),
        }
    }
}

impl std::fmt::Display for LlmProviderType {
    /// Returns PascalCase token matching the serde wire representation.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Local => f.write_str("Local"),
            Self::Remote => f.write_str("Remote"),
        }
    }
}

impl std::fmt::Display for AiAccessMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ProviderApiKey => f.write_str("provider_api_key"),
            Self::LocalModel => f.write_str("local_model"),
            Self::ProviderSubscriptionCli => f.write_str("provider_subscription_cli"),
            Self::ProviderOAuth => f.write_str("provider_oauth"),
        }
    }
}

impl std::fmt::Display for AiProviderType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Anthropic => f.write_str("anthropic"),
            Self::OpenAi => f.write_str("open_ai"),
            Self::Google => f.write_str("google"),
            Self::Ollama => f.write_str("ollama"),
            Self::Bedrock => f.write_str("bedrock"),
            Self::Copilot => f.write_str("copilot"),
            Self::Generic => f.write_str("generic"),
        }
    }
}

impl std::fmt::Display for ExternalDataPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PiiFilterStrict => f.write_str("PiiFilterStrict"),
            Self::PiiFilterStandard => f.write_str("PiiFilterStandard"),
            Self::AllowFiltered => f.write_str("AllowFiltered"),
        }
    }
}

impl std::fmt::Display for CoachingTone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Direct => f.write_str("Direct"),
            Self::Gentle => f.write_str("Gentle"),
            Self::DataDriven => f.write_str("DataDriven"),
        }
    }
}

impl std::fmt::Display for OverlayMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Minimal => f.write_str("Minimal"),
            Self::Rich => f.write_str("Rich"),
            Self::Adaptive => f.write_str("Adaptive"),
        }
    }
}

impl std::fmt::Display for MicInputMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PushToTalk => write!(f, "push_to_talk"),
            Self::VoiceActivity => write!(f, "voice_activity"),
        }
    }
}

/// Runtime gate for automation command execution.
///
/// This is the **canonical** confirmation type used throughout the codebase.
/// `AutomationConfig.confirmation_policy` previously used a separate
/// `AutomationConfirmPolicy` (UX labels: `AlwaysConfirm/TrustedOnly/NeverConfirm`),
/// which has been unified into this enum (F-RC-C24-02).
///
/// Semantic mapping from the retired enum:
/// - `AlwaysConfirm` → `Confirm`  (always show modal)
/// - `TrustedOnly`   → `Confirm`  (confirm unless trusted — default-safe)
/// - `NeverConfirm`  → `Auto`     (execute without prompt)
///
/// Serialises as `SCREAMING_SNAKE_CASE` to match the config file convention
/// established by the former `AutomationConfirmPolicy` serde attribute.
///
/// The enum-level `Default` is `Confirm` — **fail-safe**. This default is what
/// any policy deserialised without an explicit `confirmation` field receives
/// (`ExecutionPolicy.confirmation` is `#[serde(default)]`), so it must stay on
/// the safe side. The intent-hint knob (`AutomationConfig.confirmation_policy`)
/// deliberately overrides this with `Auto` at the field level (D2-② product
/// sign-off: immediate-run) — see `sections/privacy.rs`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ConfirmationRequirement {
    /// Low-risk: execute without user prompt.
    Auto,
    /// Medium-risk: show confirmation modal (fail-safe default).
    #[default]
    Confirm,
    /// High-risk: block execution entirely.
    Block,
}

impl std::fmt::Display for DataLookback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Today => f.write_str("today"),
            Self::Week => f.write_str("week"),
            Self::Month => f.write_str("month"),
        }
    }
}

impl std::fmt::Display for SttLanguage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Auto => f.write_str("auto"),
            Self::En => f.write_str("en"),
            Self::Ko => f.write_str("ko"),
        }
    }
}

impl std::fmt::Display for WhisperModelSize {
    /// Returns the lowercase machine-readable token (e.g. `"base"`).
    /// For richer UI text with size hints, use [`WhisperModelSize::display_name`].
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tiny => f.write_str("tiny"),
            Self::Base => f.write_str("base"),
            Self::Small => f.write_str("small"),
            Self::Medium => f.write_str("medium"),
        }
    }
}

impl std::fmt::Display for SttProviderKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Local => f.write_str("local"),
            Self::Cloud => f.write_str("cloud"),
        }
    }
}

impl std::fmt::Display for ConfirmationRequirement {
    /// Returns the SCREAMING_SNAKE_CASE token, matching the serde representation.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Auto => f.write_str("AUTO"),
            Self::Confirm => f.write_str("CONFIRM"),
            Self::Block => f.write_str("BLOCK"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_data_lookback_display() {
        assert_eq!(DataLookback::Today.to_string(), "today");
        assert_eq!(DataLookback::Week.to_string(), "week");
        assert_eq!(DataLookback::Month.to_string(), "month");
    }

    #[test]
    fn test_stt_language_display() {
        assert_eq!(SttLanguage::Auto.to_string(), "auto");
        assert_eq!(SttLanguage::En.to_string(), "en");
        assert_eq!(SttLanguage::Ko.to_string(), "ko");
    }

    #[test]
    fn test_whisper_model_size_display_short_token() {
        assert_eq!(WhisperModelSize::Tiny.to_string(), "tiny");
        assert_eq!(WhisperModelSize::Base.to_string(), "base");
        assert_eq!(WhisperModelSize::Small.to_string(), "small");
        assert_eq!(WhisperModelSize::Medium.to_string(), "medium");
        // Display must NOT return the verbose display_name string.
        assert_ne!(
            WhisperModelSize::Base.to_string(),
            WhisperModelSize::Base.display_name()
        );
    }

    #[test]
    fn test_stt_provider_kind_display() {
        assert_eq!(SttProviderKind::Local.to_string(), "local");
        assert_eq!(SttProviderKind::Cloud.to_string(), "cloud");
    }

    #[test]
    fn test_confirmation_requirement_display() {
        assert_eq!(ConfirmationRequirement::Auto.to_string(), "AUTO");
        assert_eq!(ConfirmationRequirement::Confirm.to_string(), "CONFIRM");
        assert_eq!(ConfirmationRequirement::Block.to_string(), "BLOCK");
    }

    #[test]
    fn confirmation_requirement_serde_round_trip_screaming_snake_case() {
        // Config files must serialise/deserialise as SCREAMING_SNAKE_CASE.
        let cases = [
            (ConfirmationRequirement::Auto, "\"AUTO\""),
            (ConfirmationRequirement::Confirm, "\"CONFIRM\""),
            (ConfirmationRequirement::Block, "\"BLOCK\""),
        ];
        for (variant, expected_json) in cases {
            let serialised = serde_json::to_string(&variant).expect("must serialise");
            assert_eq!(
                serialised, expected_json,
                "serde token mismatch for {:?}",
                variant
            );
            let deserialised: ConfirmationRequirement =
                serde_json::from_str(&serialised).expect("must deserialise");
            assert_eq!(deserialised, variant, "round-trip failed for {:?}", variant);
        }
    }

    #[test]
    fn confirmation_requirement_default_is_confirm() {
        // Enum-level default stays fail-safe (Confirm): it is what a policy
        // deserialised without an explicit `confirmation` field receives via
        // #[serde(default)]. The intent-hint knob's Auto default is applied at
        // the AutomationConfig field level instead (sections/privacy.rs).
        assert_eq!(
            ConfirmationRequirement::default(),
            ConfirmationRequirement::Confirm
        );
    }

    #[test]
    fn ocr_provider_type_serde_round_trip_pascal_case() {
        let cases = [
            (OcrProviderType::Local, "\"Local\""),
            (OcrProviderType::Remote, "\"Remote\""),
        ];
        for (variant, expected_json) in cases {
            let s = serde_json::to_string(&variant).expect("must serialise");
            assert_eq!(s, expected_json, "serde token mismatch for {:?}", variant);
            let d: OcrProviderType = serde_json::from_str(&s).expect("must deserialise");
            assert_eq!(d, variant, "round-trip failed for {:?}", variant);
            // Display must match the serde wire token.
            assert_eq!(variant.to_string(), expected_json.trim_matches('"'));
        }
    }

    #[test]
    fn llm_provider_type_serde_round_trip_pascal_case() {
        let cases = [
            (LlmProviderType::Local, "\"Local\""),
            (LlmProviderType::Remote, "\"Remote\""),
        ];
        for (variant, expected_json) in cases {
            let s = serde_json::to_string(&variant).expect("must serialise");
            assert_eq!(s, expected_json, "serde token mismatch for {:?}", variant);
            let d: LlmProviderType = serde_json::from_str(&s).expect("must deserialise");
            assert_eq!(d, variant, "round-trip failed for {:?}", variant);
            // Display must match the serde wire token.
            assert_eq!(variant.to_string(), expected_json.trim_matches('"'));
        }
    }

    #[test]
    fn external_data_policy_serde_round_trip_pascal_case() {
        let cases = [
            (ExternalDataPolicy::PiiFilterStrict, "\"PiiFilterStrict\""),
            (
                ExternalDataPolicy::PiiFilterStandard,
                "\"PiiFilterStandard\"",
            ),
            (ExternalDataPolicy::AllowFiltered, "\"AllowFiltered\""),
        ];
        for (variant, expected_json) in cases {
            let s = serde_json::to_string(&variant).expect("must serialise");
            assert_eq!(s, expected_json, "serde token mismatch for {:?}", variant);
            let d: ExternalDataPolicy = serde_json::from_str(&s).expect("must deserialise");
            assert_eq!(d, variant, "round-trip failed for {:?}", variant);
            // Display output must match the serde wire token.
            assert_eq!(variant.to_string(), expected_json.trim_matches('"'));
        }
    }

    #[test]
    fn sandbox_profile_serde_round_trip_pascal_case() {
        let cases = [
            (SandboxProfile::Permissive, "\"Permissive\""),
            (SandboxProfile::Standard, "\"Standard\""),
            (SandboxProfile::Strict, "\"Strict\""),
        ];
        for (variant, expected_json) in cases {
            let s = serde_json::to_string(&variant).expect("must serialise");
            assert_eq!(s, expected_json);
            let d: SandboxProfile = serde_json::from_str(&s).expect("must deserialise");
            assert_eq!(d, variant);
        }
    }
}
