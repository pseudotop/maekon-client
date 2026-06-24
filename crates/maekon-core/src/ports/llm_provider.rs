//! LLM provider port — defines the contract for interpreting user intent
//! from screen context via a remote large language model.
//! Implemented by `RemoteLlmProvider` in `maekon-network`.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU8, Ordering as AtomicOrdering};

use crate::error::CoreError;
use crate::models::skill::SkillMeta;

// Tri-state constants for LlmCallHealth.
const LLM_HEALTH_UNKNOWN: u8 = 0;
const LLM_HEALTH_OK: u8 = 1;
const LLM_HEALTH_FAILED: u8 = 2;

/// Per-call LLM health observable for the automation intent path.
///
/// Lives in `maekon-core` so that adapter crates (`maekon-network` as writer,
/// `maekon-web` as reader) do not need an adapter-to-adapter dependency, which
/// is forbidden by the Hexagonal Architecture rules of this workspace.
///
/// Tri-state semantics via `AtomicU8`:
/// - UNKNOWN (0): default; no call attempted yet, or handle not wired.
/// - OK      (1): last LLM call completed with a successful response.
/// - FAILED  (2): last LLM call returned a network/status/parse error.
///
/// Exposed as `Option<bool>` via `as_option_bool()`:
/// - `None`        — UNKNOWN
/// - `Some(true)`  — OK
/// - `Some(false)` — FAILED
pub struct LlmCallHealth(AtomicU8);

impl Default for LlmCallHealth {
    fn default() -> Self {
        Self(AtomicU8::new(LLM_HEALTH_UNKNOWN))
    }
}

impl LlmCallHealth {
    /// Mark the last call as successful.
    pub fn record_ok(&self) {
        self.0.store(LLM_HEALTH_OK, AtomicOrdering::Relaxed);
    }

    /// Mark the last call as failed.
    pub fn record_failed(&self) {
        self.0.store(LLM_HEALTH_FAILED, AtomicOrdering::Relaxed);
    }

    /// Convert to `Option<bool>`:
    /// - `None`         — UNKNOWN (no call yet / not wired)
    /// - `Some(true)`   — last call succeeded
    /// - `Some(false)`  — last call failed
    pub fn as_option_bool(&self) -> Option<bool> {
        match self.0.load(AtomicOrdering::Relaxed) {
            LLM_HEALTH_OK => Some(true),
            LLM_HEALTH_FAILED => Some(false),
            _ => None,
        }
    }
}

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

    /// Network endpoints that may carry screen context or intent prompts.
    ///
    /// In-process/local providers return an empty list. Providers that report
    /// `is_external() == false` while still using an endpoint must expose that
    /// URL so guard decorators can independently verify that every such
    /// endpoint is loopback before allowing an unguarded local fast path.
    fn egress_endpoint_urls(&self) -> Vec<&str> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    // ── LlmCallHealth unit tests (#5734) ─────────────────────────────────

    /// Default state is UNKNOWN — `as_option_bool()` returns `None`.
    #[test]
    fn llm_call_health_default_is_unknown() {
        let h = LlmCallHealth::default();
        assert_eq!(h.as_option_bool(), None);
    }

    /// UNKNOWN → OK transition: `as_option_bool()` returns `Some(true)`.
    #[test]
    fn llm_call_health_record_ok_transitions_to_some_true() {
        let h = LlmCallHealth::default();
        h.record_ok();
        assert_eq!(h.as_option_bool(), Some(true));
    }

    /// UNKNOWN → FAILED transition: `as_option_bool()` returns `Some(false)`.
    #[test]
    fn llm_call_health_record_failed_transitions_to_some_false() {
        let h = LlmCallHealth::default();
        h.record_failed();
        assert_eq!(h.as_option_bool(), Some(false));
    }

    /// OK → FAILED transition: subsequent failure overwrites prior success.
    #[test]
    fn llm_call_health_ok_then_failed_returns_some_false() {
        let h = LlmCallHealth::default();
        h.record_ok();
        h.record_failed();
        assert_eq!(h.as_option_bool(), Some(false));
    }

    /// FAILED → OK transition: recovery after a failure is observable.
    #[test]
    fn llm_call_health_failed_then_ok_returns_some_true() {
        let h = LlmCallHealth::default();
        h.record_failed();
        h.record_ok();
        assert_eq!(h.as_option_bool(), Some(true));
    }

    /// Arc-shared handle: both writer and reader see the same state.
    #[test]
    fn llm_call_health_arc_shared_write_visible_from_clone() {
        let writer = Arc::new(LlmCallHealth::default());
        let reader = Arc::clone(&writer);
        assert_eq!(reader.as_option_bool(), None);
        writer.record_ok();
        assert_eq!(reader.as_option_bool(), Some(true));
        writer.record_failed();
        assert_eq!(reader.as_option_bool(), Some(false));
    }

    // ── Original LlmProvider port tests ──────────────────────────────────

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
