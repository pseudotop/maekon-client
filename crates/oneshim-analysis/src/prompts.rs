/// System prompt for productivity analysis.
/// Instructs the LLM to return structured JSON suggestions.
pub const ANALYSIS_SYSTEM_PROMPT: &str = r#"You are a productivity assistant analyzing desktop work patterns.
Given the user's current activity context, provide actionable suggestions.

Respond ONLY with a JSON array of suggestions (no markdown, no explanation):
[{
  "type": "ProductivityTip",
  "content": "suggestion text",
  "confidence": 0.8,
  "reasoning": "brief explanation"
}]

Valid types: ProductivityTip, WorkflowOptimization, ContextBased, WorkGuidance
Rules:
- Max 3 suggestions per analysis
- Only suggest if confidence >= 0.6
- Be specific and actionable, not generic
- Consider the detected patterns and current focus state
- If no meaningful suggestion, return an empty array []"#;

/// A single few-shot example for the system prompt.
#[derive(Debug, Clone)]
pub struct FewShotExample {
    pub context_summary: String,
    pub suggestion_content: String,
    pub suggestion_type: String,
    pub outcome: FewShotOutcome,
}

/// Outcome label for a few-shot example.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FewShotOutcome {
    Accepted,
    Rejected,
}

/// Builds a system prompt with optional few-shot examples and regime context.
pub struct PromptBuilder {
    regime_hint: Option<String>,
    examples: Vec<FewShotExample>,
}

impl PromptBuilder {
    pub fn new() -> Self {
        Self {
            regime_hint: None,
            examples: vec![],
        }
    }

    pub fn with_regime(mut self, regime: &str) -> Self {
        self.regime_hint = Some(regime.to_string());
        self
    }

    pub fn with_examples(mut self, examples: Vec<FewShotExample>) -> Self {
        self.examples = examples;
        self
    }

    pub fn build(&self) -> String {
        let mut prompt = ANALYSIS_SYSTEM_PROMPT.to_string();

        if let Some(ref regime) = self.regime_hint {
            prompt.push_str(&format!(
                "\n\nCurrent work mode: the user is in a \"{regime}\" regime."
            ));
        }

        if !self.examples.is_empty() {
            prompt.push_str("\n\nExamples of past suggestions:");
            for ex in &self.examples {
                let label = match ex.outcome {
                    FewShotOutcome::Accepted => "Accepted",
                    FewShotOutcome::Rejected => "Rejected",
                };
                prompt.push_str(&format!(
                    "\n[{label}] Context: {}\nSuggestion ({}): {}",
                    ex.context_summary, ex.suggestion_type, ex.suggestion_content
                ));
            }
            prompt.push_str(
                "\nPrefer patterns similar to accepted examples. Avoid patterns similar to rejected examples."
            );
        }

        prompt
    }
}

impl Default for PromptBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_empty_returns_base_prompt() {
        let result = PromptBuilder::new().build();
        assert_eq!(result, ANALYSIS_SYSTEM_PROMPT);
    }

    #[test]
    fn build_with_regime_hint() {
        let result = PromptBuilder::new().with_regime("deep_focus").build();
        assert!(result.starts_with(ANALYSIS_SYSTEM_PROMPT));
        assert!(result.contains("\"deep_focus\" regime"));
    }

    #[test]
    fn build_with_one_example() {
        let example = FewShotExample {
            context_summary: "VSCode — main.rs".to_string(),
            suggestion_content: "Take a break".to_string(),
            suggestion_type: "ProductivityTip".to_string(),
            outcome: FewShotOutcome::Accepted,
        };
        let result = PromptBuilder::new().with_examples(vec![example]).build();
        assert!(result.contains("[Accepted]"));
        assert!(result.contains("VSCode — main.rs"));
        assert!(result.contains("Take a break"));
        assert!(result.contains("ProductivityTip"));
    }

    #[test]
    fn build_with_mixed_examples() {
        let examples = vec![
            FewShotExample {
                context_summary: "Slack — #general".to_string(),
                suggestion_content: "Close Slack".to_string(),
                suggestion_type: "WorkflowOptimization".to_string(),
                outcome: FewShotOutcome::Accepted,
            },
            FewShotExample {
                context_summary: "Chrome — YouTube".to_string(),
                suggestion_content: "Watch more videos".to_string(),
                suggestion_type: "ProductivityTip".to_string(),
                outcome: FewShotOutcome::Rejected,
            },
        ];
        let result = PromptBuilder::new().with_examples(examples).build();
        assert!(result.contains("[Accepted]"));
        assert!(result.contains("[Rejected]"));
        assert!(result.contains("Prefer patterns similar to accepted examples"));
        assert!(result.contains("Avoid patterns similar to rejected examples"));
    }
}
