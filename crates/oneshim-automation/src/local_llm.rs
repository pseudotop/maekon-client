use async_trait::async_trait;

use oneshim_core::error::CoreError;
use oneshim_core::ports::llm_provider::{InterpretedAction, LlmProvider, ScreenContext};

pub struct LocalLlmProvider;

impl LocalLlmProvider {
    pub fn new() -> Self {
        Self
    }
}

impl Default for LocalLlmProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl LlmProvider for LocalLlmProvider {
    async fn interpret_intent(
        &self,
        screen_context: &ScreenContext,
        intent_hint: &str,
    ) -> Result<InterpretedAction, CoreError> {
        let hint_lower = intent_hint.to_lowercase();

        let action_type = detect_action_type(&hint_lower);

        let (target_text, confidence) = find_best_match(&screen_context.visible_texts, &hint_lower);

        let target_role = infer_role(&action_type, &hint_lower);

        Ok(InterpretedAction {
            target_text,
            target_role,
            action_type,
            confidence,
        })
    }

    fn provider_name(&self) -> &str {
        "local-rule-based"
    }

    fn is_external(&self) -> bool {
        false
    }
}

fn detect_action_type(hint: &str) -> String {
    if hint.contains("클릭")
        || hint.contains("click")
        || hint.contains("누르")
        || hint.contains("press")
    {
        "click".to_string()
    } else if hint.contains("입력")
        || hint.contains("type")
        || hint.contains("작성")
        || hint.contains("write")
    {
        "type".to_string()
    } else if hint.contains("단축키") || hint.contains("hotkey") || hint.contains("shortcut") {
        "hotkey".to_string()
    } else if hint.contains("대기") || hint.contains("wait") {
        "wait".to_string()
    } else if hint.contains("active화")
        || hint.contains("activate")
        || hint.contains("열기")
        || hint.contains("open")
    {
        "activate".to_string()
    } else {
        "click".to_string() // default value
    }
}

fn find_best_match(visible_texts: &[String], hint: &str) -> (Option<String>, f64) {
    if visible_texts.is_empty() {
        return (None, 0.0);
    }

    let mut best_text: Option<String> = None;
    let mut best_score: f64 = 0.0;

    for text in visible_texts {
        let text_lower = text.to_lowercase();
        let score = simple_similarity(&text_lower, hint);
        if score > best_score {
            best_score = score;
            best_text = Some(text.clone());
        }
    }

    if best_score < 0.1 {
        (None, 0.0)
    } else {
        (best_text, best_score)
    }
}

fn simple_similarity(text: &str, hint: &str) -> f64 {
    if text == hint {
        return 1.0;
    }

    let hint_words: Vec<&str> = hint.split_whitespace().collect();
    if hint_words.is_empty() {
        return 0.0;
    }

    let matched = hint_words.iter().filter(|w| text.contains(*w)).count();

    matched as f64 / hint_words.len() as f64
}

fn infer_role(action_type: &str, hint: &str) -> Option<String> {
    match action_type {
        "click" => {
            if hint.contains("버튼") || hint.contains("button") {
                Some("button".to_string())
            } else if hint.contains("링크") || hint.contains("link") {
                Some("link".to_string())
            } else if hint.contains("메뉴") || hint.contains("menu") {
                Some("menu".to_string())
            } else {
                None
            }
        }
        "type" => {
            if hint.contains("검색") || hint.contains("search") {
                Some("search".to_string())
            } else {
                Some("input".to_string())
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_action_type_click() {
        assert_eq!(detect_action_type("click the save button"), "click");
        assert_eq!(detect_action_type("press confirm"), "click");
    }

    #[test]
    fn detect_action_type_type() {
        assert_eq!(detect_action_type("type text"), "type");
        assert_eq!(detect_action_type("type some text"), "type");
    }

    #[test]
    fn detect_action_type_hotkey() {
        assert_eq!(detect_action_type("execute hotkey"), "hotkey");
        assert_eq!(detect_action_type("hotkey Ctrl+S"), "hotkey");
    }

    #[test]
    fn detect_action_type_default() {
        assert_eq!(detect_action_type("unknown action"), "click");
    }

    #[test]
    fn find_best_match_exact() {
        let texts = vec!["save".to_string(), "edit".to_string()];
        let (text, score) = find_best_match(&texts, "save");
        assert_eq!(text.unwrap(), "save");
        assert!((score - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn find_best_match_partial() {
        let texts = vec!["file save".to_string(), "edit".to_string()];
        let (text, score) = find_best_match(&texts, "file save");
        assert_eq!(text.unwrap(), "file save");
        assert!(score > 0.0);
    }

    #[test]
    fn find_best_match_empty() {
        let texts: Vec<String> = vec![];
        let (text, score) = find_best_match(&texts, "save");
        assert!(text.is_none());
        assert!((score).abs() < f64::EPSILON);
    }

    #[test]
    fn infer_role_button() {
        assert_eq!(
            infer_role("click", "click button"),
            Some("button".to_string())
        );
        assert_eq!(
            infer_role("click", "click button"),
            Some("button".to_string())
        );
    }

    #[test]
    fn infer_role_input() {
        assert_eq!(infer_role("type", "input"), Some("input".to_string()));
        assert_eq!(
            infer_role("type", "search input"),
            Some("search".to_string())
        );
    }

    #[test]
    fn infer_role_none() {
        assert_eq!(infer_role("click", "anything"), None);
        assert_eq!(infer_role("hotkey", "shortcut"), None);
    }

    #[tokio::test]
    async fn local_llm_interpret_intent() {
        let provider = LocalLlmProvider::new();
        let ctx = ScreenContext {
            visible_texts: vec!["file".to_string(), "save".to_string(), "edit".to_string()],
            active_app: "VSCode".to_string(),
            active_window_title: "main.rs".to_string(),
            layout_description: None,
        };

        let result = provider
            .interpret_intent(&ctx, "click the save button")
            .await
            .unwrap();
        assert_eq!(result.action_type, "click");
        assert!(result.target_text.is_some());
        assert_eq!(result.target_role, Some("button".to_string()));
    }

    #[test]
    fn local_llm_provider_info() {
        let provider = LocalLlmProvider::new();
        assert_eq!(provider.provider_name(), "local-rule-based");
        assert!(!provider.is_external());
    }
}
