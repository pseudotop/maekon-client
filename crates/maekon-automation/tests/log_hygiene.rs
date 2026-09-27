//! Screen-text log hygiene for the intent resolver (#12515).
//!
//! The click and wait paths handle text read off the user's screen. Their
//! tracing events, and the not-found error message that the retry loop and the
//! GUI controller log at WARN, must name that text by length only. A small
//! field-recording subscriber is installed for the test thread, so it sees
//! every level without a formatting dependency.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use maekon_automation::intent_resolver::IntentResolver;
use maekon_automation::AutomationError;
use maekon_core::error::CoreError;
use maekon_core::models::intent::{
    AutomationIntent, ElementBounds, FinderSource, IntentConfig, UiElement,
};
use maekon_core::ports::element_finder::ElementFinder;
use maekon_core::ports::input_driver::InputDriver;

const SCREEN_TEXT: &str = "Invoice 4471 for Jane Roe";
const SENSITIVE_PART: &str = "Jane Roe";

/// Records each event's fields as ` name=value` text, one event per line.
#[derive(Clone, Default)]
struct FieldCapture(Arc<Mutex<String>>);

struct FieldLine<'a>(&'a mut String);

impl tracing::field::Visit for FieldLine<'_> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        use std::fmt::Write as _;
        let _ = write!(self.0, " {}={:?}", field.name(), value);
    }
}

impl tracing::Subscriber for FieldCapture {
    fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        let mut line = String::new();
        event.record(&mut FieldLine(&mut line));
        line.push('\n');
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push_str(&line);
    }
    fn enter(&self, _span: &tracing::span::Id) {}
    fn exit(&self, _span: &tracing::span::Id) {}
}

/// Finder that returns the same elements for every query.
struct FixedFinder(Vec<UiElement>);

#[async_trait]
impl ElementFinder for FixedFinder {
    async fn find_element(
        &self,
        _text: Option<&str>,
        _role: Option<&str>,
        _region: Option<&ElementBounds>,
    ) -> Result<Vec<UiElement>, CoreError> {
        Ok(self.0.clone())
    }
    fn name(&self) -> &str {
        "fixed"
    }
}

struct NoopDriver;

#[async_trait]
impl InputDriver for NoopDriver {
    async fn mouse_move(&self, _x: i32, _y: i32) -> Result<(), CoreError> {
        Ok(())
    }
    async fn mouse_click(&self, _button: &str, _x: i32, _y: i32) -> Result<(), CoreError> {
        Ok(())
    }
    async fn type_text(&self, _text: &str) -> Result<(), CoreError> {
        Ok(())
    }
    async fn key_press(&self, _key: &str) -> Result<(), CoreError> {
        Ok(())
    }
    async fn key_release(&self, _key: &str) -> Result<(), CoreError> {
        Ok(())
    }
    async fn hotkey(&self, _keys: &[String]) -> Result<(), CoreError> {
        Ok(())
    }
    async fn activate_app(&self, _app_name: &str) -> Result<bool, CoreError> {
        Ok(true)
    }
    fn platform(&self) -> &str {
        "noop"
    }
}

fn resolver(elements: Vec<UiElement>) -> IntentResolver {
    IntentResolver::new(
        Arc::new(FixedFinder(elements)),
        Arc::new(NoopDriver),
        IntentConfig {
            retry_interval_ms: 1,
            ..IntentConfig::default()
        },
    )
}

fn screen_element(confidence: f64) -> UiElement {
    UiElement {
        text: SCREEN_TEXT.to_string(),
        bounds: ElementBounds {
            x: 100,
            y: 100,
            width: 80,
            height: 30,
        },
        role: Some("button".to_string()),
        confidence,
        source: FinderSource::Ocr,
    }
}

fn click(text: &str) -> AutomationIntent {
    AutomationIntent::ClickElement {
        text: Some(text.to_string()),
        role: None,
        app_name: None,
        button: "left".to_string(),
    }
}

#[tokio::test]
async fn click_and_wait_paths_log_screen_text_only_as_a_length() {
    let capture = FieldCapture::default();
    let _guard = tracing::subscriber::set_default(capture.clone());

    let found = resolver(vec![screen_element(0.95)]);
    for intent in [
        click(SCREEN_TEXT),
        AutomationIntent::TypeIntoElement {
            element_text: Some(SCREEN_TEXT.to_string()),
            role: None,
            text: "typed".to_string(),
        },
        AutomationIntent::WaitForText {
            text: SCREEN_TEXT.to_string(),
            timeout_ms: 10,
        },
    ] {
        found
            .resolve_and_execute(&intent)
            .await
            .expect("the element is found");
    }
    let never_found = resolver(Vec::new());
    let wait = AutomationIntent::WaitForText {
        text: SCREEN_TEXT.to_string(),
        timeout_ms: 5,
    };
    assert!(matches!(
        never_found.resolve_and_execute(&wait).await.unwrap_err(),
        AutomationError::ExecutionTimeout { .. }
    ));

    let logs = capture.0.lock().unwrap().clone();
    for message in [
        "element click",
        " click ",
        "text waiting",
        "text found",
        "text waiting timeout",
    ] {
        assert!(logs.contains(message), "missing {message:?} in {logs}");
    }
    assert!(
        !logs.contains(SENSITIVE_PART),
        "screen text reached the log: {logs}"
    );
    assert!(
        logs.contains(&format!("text_len={}", SCREEN_TEXT.len())),
        "{logs}"
    );
}

#[tokio::test]
async fn the_not_found_message_names_the_target_only_by_length() {
    // The retry loop and the GUI controller log this message at WARN.
    let error = resolver(vec![screen_element(0.1)])
        .resolve_and_execute(&click(SCREEN_TEXT))
        .await
        .unwrap_err();
    let AutomationError::ElementNotFound(message) = error else {
        panic!("a low-confidence match is ElementNotFound, got {error:?}");
    };
    assert!(!message.contains(SENSITIVE_PART), "{message}");
    assert!(
        message.contains(&format!("text_len={}", SCREEN_TEXT.len())),
        "{message}"
    );
}
