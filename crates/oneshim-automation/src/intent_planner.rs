use crate::error::AutomationError;
use async_trait::async_trait;
use oneshim_core::error::CoreError;
use oneshim_core::models::intent::AutomationIntent;
use oneshim_core::ports::element_finder::ElementFinder;
use oneshim_core::ports::llm_provider::{
    InterpretedAction, LlmProvider, ScreenContext, SkillContext,
};
use oneshim_core::ports::skill_loader::SkillLoader;
use std::sync::Arc;

// Re-export from core — canonical definition is in oneshim-core/src/ports/intent_planner.rs
pub use oneshim_core::ports::intent_planner::IntentPlanner;

pub struct LlmIntentPlanner {
    llm_provider: Arc<dyn LlmProvider>,
    element_finder: Arc<dyn ElementFinder>,
    skill_loader: Option<Arc<dyn SkillLoader>>,
    wait_timeout_ms: u64,
}

impl LlmIntentPlanner {
    pub fn new(llm_provider: Arc<dyn LlmProvider>, element_finder: Arc<dyn ElementFinder>) -> Self {
        Self {
            llm_provider,
            element_finder,
            skill_loader: None,
            wait_timeout_ms: 5_000,
        }
    }

    /// Attach a skill loader for progressive skill disclosure.
    pub fn with_skill_loader(mut self, loader: Arc<dyn SkillLoader>) -> Self {
        self.skill_loader = Some(loader);
        self
    }

    async fn build_screen_context(&self) -> ScreenContext {
        let elements = self
            .element_finder
            .find_element(None, None, None)
            .await
            .unwrap_or_default();

        let mut visible_texts = Vec::new();
        for elem in elements {
            let trimmed = elem.text.trim();
            if trimmed.is_empty() {
                continue;
            }
            if visible_texts.len() >= 200 {
                break;
            }
            if !visible_texts.iter().any(|t| t == trimmed) {
                visible_texts.push(trimmed.to_string());
            }
        }

        ScreenContext {
            visible_texts,
            active_app: "unknown".to_string(),
            active_window_title: "unknown".to_string(),
            layout_description: None,
        }
    }

    fn action_to_intent(
        &self,
        action: InterpretedAction,
        intent_hint: &str,
    ) -> Result<AutomationIntent, AutomationError> {
        let action_type = action.action_type.to_lowercase();
        match action_type.as_str() {
            "click" => Ok(AutomationIntent::ClickElement {
                text: action.target_text,
                role: action.target_role,
                app_name: None,
                button: "left".to_string(),
            }),
            "type" => Ok(AutomationIntent::TypeIntoElement {
                element_text: action.target_text,
                role: action.target_role,
                text: extract_quoted_text(intent_hint).unwrap_or_else(|| intent_hint.to_string()),
            }),
            "hotkey" => Ok(AutomationIntent::ExecuteHotkey {
                keys: parse_hotkey_keys(intent_hint).ok_or_else(|| {
                    AutomationError::InvalidArguments(
                        "단축키 의도는 'Ctrl+S' 형태 키 조합이 필요합니다".to_string(),
                    )
                })?,
            }),
            "wait" => Ok(AutomationIntent::WaitForText {
                text: action
                    .target_text
                    .or_else(|| extract_quoted_text(intent_hint))
                    .unwrap_or_else(|| intent_hint.to_string()),
                timeout_ms: self.wait_timeout_ms,
            }),
            "activate" => Ok(AutomationIntent::ActivateApp {
                app_name: action
                    .target_text
                    .or_else(|| extract_quoted_text(intent_hint))
                    .unwrap_or_else(|| intent_hint.to_string()),
            }),
            other => Err(AutomationError::InvalidArguments(format!(
                "지원하지 않는 action_type: {other}"
            ))),
        }
    }
}

#[async_trait]
impl IntentPlanner for LlmIntentPlanner {
    async fn plan(&self, intent_hint: &str) -> Result<AutomationIntent, CoreError> {
        let screen_context = self.build_screen_context().await;

        let interpreted = if let Some(ref loader) = self.skill_loader {
            let skill_ctx = SkillContext {
                available_skills: loader.list_skills(),
                active_skill_body: None,
            };
            self.llm_provider
                .interpret_intent_with_skills(&screen_context, intent_hint, &skill_ctx)
                .await?
        } else {
            self.llm_provider
                .interpret_intent(&screen_context, intent_hint)
                .await?
        };

        self.action_to_intent(interpreted, intent_hint)
            .map_err(Into::into)
    }
}

fn extract_quoted_text(input: &str) -> Option<String> {
    let chars: Vec<char> = input.chars().collect();
    let first = chars.iter().position(|c| *c == '"' || *c == '\'')?;
    let quote_char = chars[first];
    let rest = &chars[first + 1..];
    let second = rest.iter().position(|c| *c == quote_char)?;
    let value: String = rest[..second].iter().collect();
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn parse_hotkey_keys(input: &str) -> Option<Vec<String>> {
    for token in input.split_whitespace() {
        if token.contains('+') {
            let keys: Vec<String> = token
                .split('+')
                .map(str::trim)
                .filter(|k| !k.is_empty())
                .map(normalize_key_name)
                .collect();
            if keys.len() >= 2 {
                return Some(keys);
            }
        }
    }
    None
}

fn normalize_key_name(key: &str) -> String {
    match key.to_lowercase().as_str() {
        "control" | "ctrl" => "Ctrl".to_string(),
        "command" | "cmd" | "meta" => "Cmd".to_string(),
        "option" | "alt" => "Alt".to_string(),
        "shift" => "Shift".to_string(),
        other => {
            if other.len() == 1 {
                other.to_uppercase()
            } else {
                let mut chars = other.chars();
                if let Some(first) = chars.next() {
                    let mut normalized = String::new();
                    normalized.extend(first.to_uppercase());
                    normalized.push_str(chars.as_str());
                    normalized
                } else {
                    key.to_string()
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oneshim_core::models::intent::{ElementBounds, FinderSource, UiElement};

    struct StubElementFinder;

    #[async_trait]
    impl ElementFinder for StubElementFinder {
        async fn find_element(
            &self,
            _text: Option<&str>,
            _role: Option<&str>,
            _region: Option<&ElementBounds>,
        ) -> Result<Vec<UiElement>, CoreError> {
            Ok(vec![UiElement {
                text: "save".to_string(),
                bounds: ElementBounds {
                    x: 0,
                    y: 0,
                    width: 10,
                    height: 10,
                },
                role: Some("button".to_string()),
                confidence: 0.9,
                source: FinderSource::Ocr,
            }])
        }

        fn name(&self) -> &str {
            "stub"
        }
    }

    struct StubLlmProvider {
        action: InterpretedAction,
    }

    #[async_trait]
    impl LlmProvider for StubLlmProvider {
        async fn interpret_intent(
            &self,
            _screen_context: &ScreenContext,
            _intent_hint: &str,
        ) -> Result<InterpretedAction, CoreError> {
            Ok(self.action.clone())
        }

        fn provider_name(&self) -> &str {
            "stub-llm"
        }

        fn is_external(&self) -> bool {
            false
        }
    }

    #[tokio::test]
    async fn plan_click_action_to_click_intent() {
        let planner = LlmIntentPlanner::new(
            Arc::new(StubLlmProvider {
                action: InterpretedAction {
                    target_text: Some("save".to_string()),
                    target_role: Some("button".to_string()),
                    action_type: "click".to_string(),
                    confidence: 0.95,
                },
            }),
            Arc::new(StubElementFinder),
        );

        let intent = planner.plan("save 버튼 클릭").await.unwrap();
        assert!(matches!(intent, AutomationIntent::ClickElement { .. }));
    }

    #[tokio::test]
    async fn plan_hotkey_action_parses_keys_from_hint() {
        let planner = LlmIntentPlanner::new(
            Arc::new(StubLlmProvider {
                action: InterpretedAction {
                    target_text: None,
                    target_role: None,
                    action_type: "hotkey".to_string(),
                    confidence: 0.8,
                },
            }),
            Arc::new(StubElementFinder),
        );

        let intent = planner.plan("Ctrl+Shift+S execution").await.unwrap();
        match intent {
            AutomationIntent::ExecuteHotkey { keys } => {
                assert_eq!(keys, vec!["Ctrl", "Shift", "S"]);
            }
            other => panic!("Unexpected intent: {other:?}"),
        }
    }

    #[test]
    fn quoted_text_extraction() {
        assert_eq!(
            extract_quoted_text("입력창에 \"hello world\" 입력"),
            Some("hello world".to_string())
        );
        assert_eq!(extract_quoted_text("no quoted text"), None);
    }
}
