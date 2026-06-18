use std::sync::Arc;
use std::time::Instant;

use tracing::{debug, info, warn};

use crate::error::AutomationError;
use maekon_core::models::intent::{
    AutomationIntent, IntentConfig, IntentResult, UiElement, VerificationResult,
};
use maekon_core::ports::element_finder::ElementFinder;
use maekon_core::ports::input_driver::InputDriver;

/// Reject a synthesized input action whose text/key count exceeds the shared
/// cap. The action_dispatcher inline path caps its own synthesis (review4 A8);
/// this closes the same hole on the IntentResolver (template/LLM) path, which
/// drives the real input driver directly and was previously uncapped (review4
/// re-verify).
fn enforce_synth_input_len(len: usize, what: &str) -> Result<(), AutomationError> {
    if len > crate::input_driver::MAX_SYNTHESIZED_INPUT_LEN {
        return Err(AutomationError::InvalidArguments(format!(
            "{what} exceeds {} (synthesized input cap)",
            crate::input_driver::MAX_SYNTHESIZED_INPUT_LEN
        )));
    }
    Ok(())
}

pub struct IntentResolver {
    element_finder: Arc<dyn ElementFinder>,
    input_driver: Arc<dyn InputDriver>,
    config: IntentConfig,
}

impl IntentResolver {
    pub fn new(
        element_finder: Arc<dyn ElementFinder>,
        input_driver: Arc<dyn InputDriver>,
        config: IntentConfig,
    ) -> Self {
        Self {
            element_finder,
            input_driver,
            config,
        }
    }

    pub async fn resolve_and_execute(
        &self,
        intent: &AutomationIntent,
    ) -> Result<(bool, Option<UiElement>), AutomationError> {
        match intent {
            AutomationIntent::ClickElement {
                text,
                role,
                app_name: _,
                button,
            } => {
                let elements = self
                    .element_finder
                    .find_element(text.as_deref(), role.as_deref(), None)
                    .await?;

                let best = elements
                    .into_iter()
                    .find(|e| e.confidence >= self.config.min_confidence)
                    .ok_or_else(|| {
                        AutomationError::ElementNotFound(format!(
                            "no element found at or above {:.0}% confidence (text={:?}, role={:?})",
                            self.config.min_confidence * 100.0,
                            text,
                            role
                        ))
                    })?;

                let (cx, cy) = best.bounds.center();
                debug!(text = %best.text, x = cx, y = cy, confidence = best.confidence, "element click");
                self.input_driver.mouse_click(button, cx, cy).await?;

                Ok((true, Some(best)))
            }

            AutomationIntent::TypeIntoElement {
                element_text,
                role,
                text,
            } => {
                let elements = self
                    .element_finder
                    .find_element(element_text.as_deref(), role.as_deref(), None)
                    .await?;

                let best = elements
                    .into_iter()
                    .find(|e| e.confidence >= self.config.min_confidence);

                if let Some(elem) = &best {
                    let (cx, cy) = elem.bounds.center();
                    debug!(text = %elem.text, x = cx, y = cy, "click");
                    self.input_driver.mouse_click("left", cx, cy).await?;
                }

                debug!(text_len = text.len(), "text");
                enforce_synth_input_len(text.chars().count(), "TypeIntoElement text")?;
                self.input_driver.type_text(text).await?;

                Ok((true, best))
            }

            AutomationIntent::ExecuteHotkey { keys } => {
                debug!(?keys, "key execution");
                enforce_synth_input_len(keys.len(), "ExecuteHotkey keys")?;
                self.input_driver.hotkey(keys).await?;
                Ok((true, None))
            }

            AutomationIntent::WaitForText { text, timeout_ms } => {
                // Clamp the untrusted preset/LLM-config wait so a malformed
                // timeout_ms cannot busy-wait the workflow task effectively forever
                // (review4 re-verify: A19 sibling — A19 clamped the inter-step delay
                // but left this unbounded).
                const MAX_WAIT_MS: u64 = 300_000;
                let effective_timeout_ms = (*timeout_ms).min(MAX_WAIT_MS);
                debug!(text, timeout_ms = effective_timeout_ms, "text waiting");
                let start = Instant::now();
                let timeout = std::time::Duration::from_millis(effective_timeout_ms);

                loop {
                    let elements = self
                        .element_finder
                        .find_element(Some(text), None, None)
                        .await;

                    if let Ok(elems) = &elements {
                        if !elems.is_empty() {
                            info!(text, elapsed_ms = start.elapsed().as_millis(), "text found");
                            return Ok((true, elems.first().cloned()));
                        }
                    }

                    if start.elapsed() >= timeout {
                        warn!(
                            text,
                            timeout_ms = effective_timeout_ms,
                            "text waiting timeout"
                        );
                        return Err(AutomationError::ExecutionTimeout {
                            timeout_ms: effective_timeout_ms,
                        });
                    }

                    tokio::time::sleep(std::time::Duration::from_millis(
                        self.config.retry_interval_ms,
                    ))
                    .await;
                }
            }

            AutomationIntent::ActivateApp { app_name } => {
                // review4 A9: app activation is NOT implemented — there is no
                // platform call here (the InputDriver port has no activate_app), so
                // returning Ok((true,..)) fabricated success and (with
                // verify_after_action) a phantom screen-change. Report success=false
                // so the executor does not claim a focus switch that never happened
                // and stop_on_failure presets halt before synthesizing input against
                // the wrong (un-switched) focused window.
                // TODO: implement real activation (macOS osascript / Windows
                // SetForegroundWindow / Linux wmctrl) behind the InputDriver port.
                warn!(
                    app_name,
                    "ActivateApp is not implemented (no-op) — reporting failure"
                );
                Ok((false, None))
            }

            AutomationIntent::Raw(action) => {
                // #5980: redact KeyType text so typed PII never reaches the log sink.
                debug!(action = %crate::sandbox::redact_action(action), "execution");
                match action {
                    maekon_core::models::automation::AutomationAction::MouseMove { x, y } => {
                        self.input_driver.mouse_move(*x, *y).await?;
                    }
                    maekon_core::models::automation::AutomationAction::MouseClick {
                        button,
                        x,
                        y,
                    } => {
                        self.input_driver.mouse_click(button, *x, *y).await?;
                    }
                    maekon_core::models::automation::AutomationAction::KeyType { text } => {
                        enforce_synth_input_len(text.chars().count(), "Raw KeyType text")?;
                        self.input_driver.type_text(text).await?;
                    }
                    maekon_core::models::automation::AutomationAction::KeyPress { key } => {
                        self.input_driver.key_press(key).await?;
                    }
                    maekon_core::models::automation::AutomationAction::KeyRelease { key } => {
                        self.input_driver.key_release(key).await?;
                    }
                    maekon_core::models::automation::AutomationAction::Hotkey { keys } => {
                        enforce_synth_input_len(keys.len(), "Raw Hotkey keys")?;
                        self.input_driver.hotkey(keys).await?;
                    }
                }
                Ok((true, None))
            }
        }
    }
}

pub struct IntentExecutor {
    resolver: IntentResolver,
    config: IntentConfig,
}

impl IntentExecutor {
    pub fn new(resolver: IntentResolver, config: IntentConfig) -> Self {
        Self { resolver, config }
    }

    pub fn with_overrides(
        &self,
        input_driver: Option<Arc<dyn InputDriver>>,
        config: Option<IntentConfig>,
    ) -> Self {
        let config = config.unwrap_or_else(|| self.config.clone());
        let resolver = IntentResolver::new(
            self.resolver.element_finder.clone(),
            input_driver.unwrap_or_else(|| self.resolver.input_driver.clone()),
            config.clone(),
        );
        Self::new(resolver, config)
    }

    pub async fn execute(
        &self,
        intent: &AutomationIntent,
    ) -> Result<IntentResult, AutomationError> {
        let start = Instant::now();
        let mut retry_count = 0u32;
        let mut last_error: Option<String> = None;

        for attempt in 0..=self.config.max_retries {
            if attempt > 0 {
                debug!(attempt, max = self.config.max_retries, "attempt");
                tokio::time::sleep(std::time::Duration::from_millis(
                    self.config.retry_interval_ms,
                ))
                .await;
            }

            match self.resolver.resolve_and_execute(intent).await {
                Ok((success, element)) => {
                    let elapsed_ms = start.elapsed().as_millis() as u64;

                    let verification = if self.config.verify_after_action {
                        tokio::time::sleep(std::time::Duration::from_millis(
                            self.config.verify_delay_ms,
                        ))
                        .await;

                        Some(VerificationResult {
                            screen_changed: success,
                            changed_regions: if success { 1 } else { 0 },
                            text_found: None,
                        })
                    } else {
                        None
                    };

                    return Ok(IntentResult {
                        success,
                        element,
                        verification,
                        retry_count,
                        elapsed_ms,
                        error: None,
                    });
                }
                Err(e) => {
                    warn!(attempt, error = %e, "execution failure");
                    last_error = Some(e.to_string());
                    retry_count = attempt;
                }
            }
        }

        let elapsed_ms = start.elapsed().as_millis() as u64;
        Ok(IntentResult {
            success: false,
            element: None,
            verification: None,
            retry_count,
            elapsed_ms,
            error: last_error,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maekon_core::error::CoreError;
    use maekon_core::models::automation::AutomationAction;
    use maekon_core::models::intent::{ElementBounds, FinderSource};

    // Mock ElementFinder
    struct MockElementFinder {
        results: Vec<UiElement>,
    }

    #[async_trait::async_trait]
    impl ElementFinder for MockElementFinder {
        async fn find_element(
            &self,
            _text: Option<&str>,
            _role: Option<&str>,
            _region: Option<&ElementBounds>,
        ) -> Result<Vec<UiElement>, CoreError> {
            Ok(self.results.clone())
        }
        fn name(&self) -> &str {
            "mock"
        }
    }

    struct EmptyElementFinder;

    #[async_trait::async_trait]
    impl ElementFinder for EmptyElementFinder {
        async fn find_element(
            &self,
            _text: Option<&str>,
            _role: Option<&str>,
            _region: Option<&ElementBounds>,
        ) -> Result<Vec<UiElement>, CoreError> {
            Ok(vec![])
        }
        fn name(&self) -> &str {
            "empty"
        }
    }

    // Mock InputDriver
    struct MockInputDriver;

    #[async_trait::async_trait]
    impl InputDriver for MockInputDriver {
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
        fn platform(&self) -> &str {
            "mock"
        }
    }

    fn make_resolver_with_elements(elements: Vec<UiElement>) -> IntentResolver {
        IntentResolver::new(
            Arc::new(MockElementFinder { results: elements }),
            Arc::new(MockInputDriver),
            IntentConfig::default(),
        )
    }

    fn make_element(text: &str, confidence: f64) -> UiElement {
        UiElement {
            text: text.to_string(),
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

    #[tokio::test]
    async fn resolve_click_element_success() {
        let resolver = make_resolver_with_elements(vec![make_element("save", 0.95)]);
        let intent = AutomationIntent::ClickElement {
            text: Some("save".to_string()),
            role: None,
            app_name: None,
            button: "left".to_string(),
        };
        let (success, element) = resolver.resolve_and_execute(&intent).await.unwrap();
        assert!(success);
        assert_eq!(element.unwrap().text, "save");
    }

    #[tokio::test]
    async fn resolve_click_element_low_confidence() {
        let resolver = make_resolver_with_elements(vec![make_element("save", 0.3)]);
        let intent = AutomationIntent::ClickElement {
            text: Some("save".to_string()),
            role: None,
            app_name: None,
            button: "left".to_string(),
        };
        let err = resolver.resolve_and_execute(&intent).await.unwrap_err();
        assert!(
            matches!(err, AutomationError::ElementNotFound(_)),
            "low-confidence element must produce ElementNotFound, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn resolve_type_into_element() {
        // Element/search text is incidental test data ("search"), ASCII-escaped to
        // keep the source ASCII while preserving the exact bytes under test.
        let resolver = make_resolver_with_elements(vec![make_element("\u{ac80}\u{c0c9}", 0.9)]);
        let intent = AutomationIntent::TypeIntoElement {
            element_text: Some("\u{ac80}\u{c0c9}".to_string()),
            role: None,
            text: "hello world".to_string(),
        };
        let (success, _) = resolver.resolve_and_execute(&intent).await.unwrap();
        assert!(success);
    }

    #[tokio::test]
    async fn resolve_execute_hotkey() {
        let resolver = make_resolver_with_elements(vec![]);
        let intent = AutomationIntent::ExecuteHotkey {
            keys: vec!["Ctrl".to_string(), "S".to_string()],
        };
        let (success, element) = resolver.resolve_and_execute(&intent).await.unwrap();
        assert!(success);
        assert!(element.is_none());
    }

    #[tokio::test]
    async fn resolve_raw_action() {
        let resolver = make_resolver_with_elements(vec![]);
        let intent = AutomationIntent::Raw(AutomationAction::MouseClick {
            button: "left".to_string(),
            x: 100,
            y: 200,
        });
        let (success, _) = resolver.resolve_and_execute(&intent).await.unwrap();
        assert!(success);
    }

    #[tokio::test]
    async fn executor_retries_on_failure() {
        let resolver = IntentResolver::new(
            Arc::new(EmptyElementFinder),
            Arc::new(MockInputDriver),
            IntentConfig {
                max_retries: 1,
                retry_interval_ms: 10,
                verify_after_action: false,
                ..IntentConfig::default()
            },
        );
        let executor = IntentExecutor::new(
            resolver,
            IntentConfig {
                max_retries: 1,
                retry_interval_ms: 10,
                verify_after_action: false,
                ..IntentConfig::default()
            },
        );

        // Target text is incidental test data ("does not exist"), ASCII-escaped to
        // keep the source ASCII while preserving the exact bytes under test.
        let intent = AutomationIntent::ClickElement {
            text: Some("\u{c874}\u{c7ac}\u{d558}\u{c9c0}\u{c54a}\u{c74c}".to_string()),
            role: None,
            app_name: None,
            button: "left".to_string(),
        };

        let result = executor.execute(&intent).await.unwrap();
        assert!(!result.success);
        assert!(result.error.is_some());
        assert!(result.retry_count >= 1);
    }

    #[tokio::test]
    async fn executor_succeeds_with_verification() {
        // Target text is incidental test data ("OK"/"confirm"), ASCII-escaped to keep
        // the source ASCII while preserving the exact bytes under test.
        let resolver = make_resolver_with_elements(vec![make_element("\u{d655}\u{c778}", 0.9)]);
        let config = IntentConfig {
            verify_after_action: true,
            verify_delay_ms: 10, // fast verification in test
            ..IntentConfig::default()
        };
        let executor = IntentExecutor::new(resolver, config);

        let intent = AutomationIntent::ClickElement {
            text: Some("\u{d655}\u{c778}".to_string()),
            role: None,
            app_name: None,
            button: "left".to_string(),
        };

        let result = executor.execute(&intent).await.unwrap();
        assert!(result.success);
        assert!(result.verification.is_some());
        assert!(result.verification.unwrap().screen_changed);
    }

    #[tokio::test]
    async fn executor_activate_app() {
        let resolver = make_resolver_with_elements(vec![]);
        let executor = IntentExecutor::new(
            resolver,
            IntentConfig {
                verify_after_action: false,
                ..IntentConfig::default()
            },
        );

        let intent = AutomationIntent::ActivateApp {
            app_name: "Visual Studio Code".to_string(),
        };

        // review4 A9: ActivateApp is not implemented (no platform call exists), so
        // it must report success=false rather than fabricate success. Previously it
        // returned Ok((true, ..)), which (with verification) also fabricated a
        // phantom screen-change and let presets continue feeding input to the wrong
        // window.
        let result = executor.execute(&intent).await.unwrap();
        assert!(
            !result.success,
            "unimplemented ActivateApp must report failure"
        );
    }

    #[tokio::test]
    async fn executor_activate_app_does_not_fabricate_screen_change() {
        // Even when verification runs, an unimplemented ActivateApp must not claim a
        // screen change happened (review4 A9 regression guard).
        let resolver = make_resolver_with_elements(vec![]);
        let executor = IntentExecutor::new(
            resolver,
            IntentConfig {
                verify_after_action: true,
                verify_delay_ms: 0,
                ..IntentConfig::default()
            },
        );
        let intent = AutomationIntent::ActivateApp {
            app_name: "Visual Studio Code".to_string(),
        };
        let result = executor.execute(&intent).await.unwrap();
        assert!(!result.success);
        let verification = result.verification.expect("verification runs when enabled");
        assert!(
            !verification.screen_changed,
            "must not fabricate a screen change for an unimplemented action"
        );
        assert_eq!(verification.changed_regions, 0);
    }

    #[tokio::test]
    async fn oversized_synthesized_keytype_is_rejected() {
        // review4 re-verify (A8 completion): the keystroke-burst cap must hold on
        // the IntentResolver synthesis path, not only the action_dispatcher inline
        // path. An over-cap Raw KeyType must error before reaching the driver.
        let resolver = make_resolver_with_elements(vec![]);
        let huge = "a".repeat(crate::input_driver::MAX_SYNTHESIZED_INPUT_LEN + 1);
        let intent =
            AutomationIntent::Raw(maekon_core::models::automation::AutomationAction::KeyType {
                text: huge,
            });
        let err = resolver
            .resolve_and_execute(&intent)
            .await
            .expect_err("oversized synthesized KeyType must be rejected");
        assert!(matches!(err, AutomationError::InvalidArguments(_)));
    }

    #[tokio::test]
    async fn synthesized_keytype_at_cap_is_accepted() {
        let resolver = make_resolver_with_elements(vec![]);
        let at_cap = "a".repeat(crate::input_driver::MAX_SYNTHESIZED_INPUT_LEN);
        let intent =
            AutomationIntent::Raw(maekon_core::models::automation::AutomationAction::KeyType {
                text: at_cap,
            });
        let (success, _) = resolver
            .resolve_and_execute(&intent)
            .await
            .expect("input exactly at the cap must be accepted");
        assert!(success);
    }

    #[tokio::test]
    async fn wait_for_text_timeout_returns_error() {
        let resolver = IntentResolver::new(
            Arc::new(EmptyElementFinder),
            Arc::new(MockInputDriver),
            IntentConfig {
                retry_interval_ms: 10,
                ..IntentConfig::default()
            },
        );
        // Wait-for text is incidental test data ("nonexistent text"), ASCII-escaped
        // to keep the source ASCII while preserving the exact bytes under test.
        let intent = AutomationIntent::WaitForText {
            text: "\u{c874}\u{c7ac}\u{d558}\u{c9c0}\u{c54a}\u{b294}\u{d14d}\u{c2a4}\u{d2b8}"
                .to_string(),
            timeout_ms: 50,
        };
        let err = resolver.resolve_and_execute(&intent).await.unwrap_err();
        assert!(
            matches!(err, AutomationError::ExecutionTimeout { .. }),
            "wait-for-text timeout must produce ExecutionTimeout, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn wait_for_text_found_immediately() {
        let resolver = make_resolver_with_elements(vec![make_element("completed", 0.9)]);
        let intent = AutomationIntent::WaitForText {
            text: "completed".to_string(),
            timeout_ms: 1000,
        };
        let (success, element) = resolver.resolve_and_execute(&intent).await.unwrap();
        assert!(success);
        assert!(element.is_some());
    }

    #[tokio::test]
    async fn click_element_not_found_returns_error() {
        let resolver = IntentResolver::new(
            Arc::new(EmptyElementFinder),
            Arc::new(MockInputDriver),
            IntentConfig::default(),
        );
        // Target text is incidental test data ("without button"), ASCII-escaped to
        // keep the source ASCII while preserving the exact bytes under test.
        let intent = AutomationIntent::ClickElement {
            text: Some("without\u{bc84}\u{d2bc}".to_string()),
            role: None,
            app_name: None,
            button: "left".to_string(),
        };
        let err = resolver.resolve_and_execute(&intent).await.unwrap_err();
        assert!(
            matches!(err, AutomationError::ElementNotFound(_)),
            "element not found must produce ElementNotFound, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn resolve_raw_key_type_action() {
        let resolver = make_resolver_with_elements(vec![]);
        let intent = AutomationIntent::Raw(AutomationAction::KeyType {
            text: "hello world".to_string(),
        });
        let (success, _) = resolver.resolve_and_execute(&intent).await.unwrap();
        assert!(success);
    }

    #[tokio::test]
    async fn resolve_raw_mouse_move_action() {
        let resolver = make_resolver_with_elements(vec![]);
        let intent = AutomationIntent::Raw(AutomationAction::MouseMove { x: 500, y: 300 });
        let (success, _) = resolver.resolve_and_execute(&intent).await.unwrap();
        assert!(success);
    }

    #[tokio::test]
    async fn executor_no_verification_success() {
        // Target text is incidental test data ("OK"/"confirm"), ASCII-escaped to keep
        // the source ASCII while preserving the exact bytes under test.
        let resolver = make_resolver_with_elements(vec![make_element("\u{d655}\u{c778}", 0.9)]);
        let config = IntentConfig {
            verify_after_action: false,
            ..IntentConfig::default()
        };
        let executor = IntentExecutor::new(resolver, config);

        let intent = AutomationIntent::ClickElement {
            text: Some("\u{d655}\u{c778}".to_string()),
            role: None,
            app_name: None,
            button: "left".to_string(),
        };

        let result = executor.execute(&intent).await.unwrap();
        assert!(result.success);
        assert!(result.verification.is_none());
        assert_eq!(result.retry_count, 0);
    }
}
