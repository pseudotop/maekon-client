//! Coaching LLM personalization prompt helpers.

/// System prompt for coaching message personalization.
pub(crate) const COACHING_SYSTEM_PROMPT: &str =
    "You are a concise productivity coach. Rewrite the given message \
     to be more personalized and contextual. Keep the same intent. \
     Respond with ONLY the rewritten message, no preamble.";

pub(crate) fn build_personalization_prompt(template_text: &str, regime_label: &str) -> String {
    format!(
        "Rewrite this productivity coaching message to be more personalized \
         and contextual. Keep the same intent and information, but make it \
         feel natural.\n\n\
         Original: {template_text}\n\
         Current regime: {regime_label}\n\
         Respond with ONLY the rewritten message, no preamble.",
    )
}
