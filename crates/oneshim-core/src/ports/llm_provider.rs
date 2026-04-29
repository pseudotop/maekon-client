//! LLM provider port — defines the contract for interpreting user intent
//! from screen context via a remote large language model.
//! Implemented by `RemoteLlmProvider` in `oneshim-network`.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::models::skill::SkillMeta;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenContext {
    pub visible_texts: Vec<String>,
    pub active_app: String,
    pub active_window_title: String,
    pub layout_description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterpretedAction {
    pub target_text: Option<String>,
    pub target_role: Option<String>,
    pub action_type: String,
    pub confidence: f64,
}

/// Optional skill context injected into the system prompt.
#[derive(Debug, Clone, Default)]
pub struct SkillContext {
    /// Available skill summaries (progressive disclosure — names only).
    pub available_skills: Vec<SkillMeta>,
    /// Activated skill body to inject fully into the prompt.
    pub active_skill_body: Option<String>,
}

/// LLM provider port — interprets user intent from screen context.
///
/// # Errors
/// - `CoreError::Analysis` (wire: `provider.analysis_failed`) for LLM-side
///   failures: empty response, non-parseable intent output, malformed JSON.
/// - HTTP-layer failures follow the canonical semantic status mapping:
///   `CoreError::Auth` (401/403), `CoreError::RequestTimeout` (408/504),
///   `CoreError::RateLimit` (429), `CoreError::ServiceUnavailable` (502/503).
///   See `docs/guides/http-status-error-mapping.md`.
/// - `CoreError::Config` with `ConfigCode::UnsupportedProviderBedrock`
///   (wire: `provider.bedrock.unsupported`) when an adapter resolves an
///   AWS Bedrock provider — AWS SigV4 is intentionally unsupported per
///   ADR-019 §3 (re-introduction requires the §5 8-step checklist).
/// - `CoreError::Network` (wire: `network.generic`) for pre-response
///   transport failures (DNS, connection refused).
#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn interpret_intent(
        &self,
        screen_context: &ScreenContext,
        intent_hint: &str,
    ) -> Result<InterpretedAction, CoreError>;

    /// Interpret intent with optional skill context injected into the prompt.
    async fn interpret_intent_with_skills(
        &self,
        screen_context: &ScreenContext,
        intent_hint: &str,
        _skill_ctx: &SkillContext,
    ) -> Result<InterpretedAction, CoreError> {
        // Default: ignore skill context, delegate to base method.
        self.interpret_intent(screen_context, intent_hint).await
    }

    fn provider_name(&self) -> &str;

    fn is_external(&self) -> bool;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_context_serde() {
        let ctx = ScreenContext {
            visible_texts: vec!["file".to_string(), "edit".to_string()],
            active_app: "Visual Studio Code".to_string(),
            active_window_title: "main.rs — VSCode".to_string(),
            layout_description: None,
        };
        let json = serde_json::to_string(&ctx).unwrap();
        let deser: ScreenContext = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.visible_texts.len(), 2);
        assert_eq!(deser.active_app, "Visual Studio Code");
    }

    #[test]
    fn interpreted_action_serde() {
        let action = InterpretedAction {
            target_text: Some("save".to_string()),
            target_role: Some("button".to_string()),
            action_type: "click".to_string(),
            confidence: 0.85,
        };
        let json = serde_json::to_string(&action).unwrap();
        let deser: InterpretedAction = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.target_text.unwrap(), "save");
        assert!((deser.confidence - 0.85).abs() < f64::EPSILON);
    }
}
