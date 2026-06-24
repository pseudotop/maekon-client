use async_trait::async_trait;
use tracing::debug;
#[cfg(feature = "enigo")]
use tracing::warn;

use maekon_core::error::CoreError;
use maekon_core::models::automation::MouseButton;
use maekon_core::models::intent::{ElementBounds, UiElement};
use maekon_core::ports::element_finder::ElementFinder;
use maekon_core::ports::input_driver::InputDriver;

/// Maximum characters/keys a single synthesized input action may carry.
///
/// Bounds untrusted preset/LLM/template-driven input so one
/// TypeIntoElement/KeyType/Hotkey cannot emit an unbounded keystroke burst
/// (review4 A8 + re-verify). Shared by `action_dispatcher` and `intent_resolver`
/// so the cap cannot diverge across the two synthesis paths.
pub(crate) const MAX_SYNTHESIZED_INPUT_LEN: usize = 4096;

pub struct NoOpInputDriver;

#[async_trait]
impl InputDriver for NoOpInputDriver {
    async fn mouse_move(&self, x: i32, y: i32) -> Result<(), CoreError> {
        debug!(x, y, "[NoOp] mouse");
        Ok(())
    }

    async fn mouse_click(&self, button: &str, x: i32, y: i32) -> Result<(), CoreError> {
        debug!(button, x, y, "[NoOp] mouse click");
        Ok(())
    }

    async fn type_text(&self, text: &str) -> Result<(), CoreError> {
        debug!(text_len = text.len(), "[NoOp] text");
        Ok(())
    }

    async fn key_press(&self, key: &str) -> Result<(), CoreError> {
        debug!(key, "[NoOp] key");
        Ok(())
    }

    async fn key_release(&self, key: &str) -> Result<(), CoreError> {
        debug!(key, "[NoOp] key");
        Ok(())
    }

    async fn hotkey(&self, keys: &[String]) -> Result<(), CoreError> {
        debug!(?keys, "[NoOp] key execution");
        Ok(())
    }

    fn platform(&self) -> &str {
        "noop"
    }
}

pub struct NoOpElementFinder;

#[async_trait]
impl ElementFinder for NoOpElementFinder {
    async fn find_element(
        &self,
        _text: Option<&str>,
        _role: Option<&str>,
        _region: Option<&ElementBounds>,
    ) -> Result<Vec<UiElement>, CoreError> {
        debug!("[NoOp] element lookup ( )");
        Ok(vec![])
    }

    fn name(&self) -> &str {
        "noop"
    }
}

#[cfg(feature = "enigo")]
pub struct EnigoInputDriver {
    enigo: tokio::sync::Mutex<enigo::Enigo>,
}

#[cfg(feature = "enigo")]
impl EnigoInputDriver {
    pub fn new() -> Result<Self, crate::error::AutomationError> {
        let settings = enigo::Settings::default();
        let enigo = enigo::Enigo::new(&settings).map_err(|e| {
            crate::error::AutomationError::Internal(format!(
                "Failed to initialize input driver: {e}"
            ))
        })?;
        Ok(Self {
            enigo: tokio::sync::Mutex::new(enigo),
        })
    }

    fn parse_key(key: &str) -> Result<enigo::Key, CoreError> {
        Ok(match key.to_lowercase().as_str() {
            "enter" | "return" => enigo::Key::Return,
            "tab" => enigo::Key::Tab,
            "escape" | "esc" => enigo::Key::Escape,
            "backspace" => enigo::Key::Backspace,
            "delete" | "del" => enigo::Key::Delete,
            "space" => enigo::Key::Space,
            "home" => enigo::Key::Home,
            "end" => enigo::Key::End,
            "pageup" => enigo::Key::PageUp,
            "pagedown" => enigo::Key::PageDown,
            "up" | "uparrow" => enigo::Key::UpArrow,
            "down" | "downarrow" => enigo::Key::DownArrow,
            "left" | "leftarrow" => enigo::Key::LeftArrow,
            "right" | "rightarrow" => enigo::Key::RightArrow,
            "ctrl" | "control" => enigo::Key::Control,
            "shift" => enigo::Key::Shift,
            "alt" | "option" => enigo::Key::Alt,
            "meta" | "command" | "cmd" | "super" | "win" => enigo::Key::Meta,
            "capslock" => enigo::Key::CapsLock,
            "f1" => enigo::Key::F1,
            "f2" => enigo::Key::F2,
            "f3" => enigo::Key::F3,
            "f4" => enigo::Key::F4,
            "f5" => enigo::Key::F5,
            "f6" => enigo::Key::F6,
            "f7" => enigo::Key::F7,
            "f8" => enigo::Key::F8,
            "f9" => enigo::Key::F9,
            "f10" => enigo::Key::F10,
            "f11" => enigo::Key::F11,
            "f12" => enigo::Key::F12,
            other => {
                // Only single-character strings map to a Unicode key. Unknown multi-char
                // key names are rejected as an error instead of silently synthesizing
                // Key::Unicode('a') — a wrong/destructive keystroke (oneshim#6124, parity
                // with the sandbox-worker hardening in oneshim#5981).
                let mut chars = other.chars();
                match (chars.next(), chars.next()) {
                    (Some(ch), None) => enigo::Key::Unicode(ch),
                    _ => {
                        warn!("rejected unknown key: {key}");
                        return Err(CoreError::InvalidArguments {
                            code: maekon_core::error_codes::ValidationCode::InvalidArguments,
                            message: format!("unknown key: {key}"),
                        });
                    }
                }
            }
        })
    }
}

#[cfg(feature = "enigo")]
#[async_trait]
impl InputDriver for EnigoInputDriver {
    async fn mouse_move(&self, x: i32, y: i32) -> Result<(), CoreError> {
        use enigo::Mouse;
        debug!(x, y, "[Enigo] mouse");
        let mut enigo = self.enigo.lock().await;
        enigo
            .move_mouse(x, y, enigo::Coordinate::Abs)
            .map_err(|e| CoreError::Internal {
                code: maekon_core::error_codes::InternalCode::Generic,
                message: format!("Mouse move failed: {e}"),
            })?;
        Ok(())
    }

    async fn mouse_click(&self, button: &str, x: i32, y: i32) -> Result<(), CoreError> {
        use enigo::Mouse;
        debug!(button, x, y, "[Enigo] mouse click");
        let mut enigo = self.enigo.lock().await;
        enigo
            .move_mouse(x, y, enigo::Coordinate::Abs)
            .map_err(|e| CoreError::Internal {
                code: maekon_core::error_codes::InternalCode::Generic,
                message: format!("Mouse move failed: {e}"),
            })?;
        let btn = match parse_mouse_button(button)? {
            MouseButton::Right => enigo::Button::Right,
            MouseButton::Middle => enigo::Button::Middle,
            MouseButton::Left => enigo::Button::Left,
        };
        enigo
            .button(btn, enigo::Direction::Click)
            .map_err(|e| CoreError::Internal {
                code: maekon_core::error_codes::InternalCode::Generic,
                message: format!("Mouse click failed: {e}"),
            })?;
        Ok(())
    }

    async fn type_text(&self, text: &str) -> Result<(), CoreError> {
        use enigo::Keyboard;
        debug!(text_len = text.len(), "[Enigo] text");
        let mut enigo = self.enigo.lock().await;
        enigo.text(text).map_err(|e| CoreError::Internal {
            code: maekon_core::error_codes::InternalCode::Generic,
            message: format!("Text input failed: {e}"),
        })?;
        Ok(())
    }

    async fn key_press(&self, key: &str) -> Result<(), CoreError> {
        use enigo::Keyboard;
        debug!(key, "[Enigo] key");
        let mut enigo = self.enigo.lock().await;
        enigo
            .key(Self::parse_key(key)?, enigo::Direction::Press)
            .map_err(|e| CoreError::Internal {
                code: maekon_core::error_codes::InternalCode::Generic,
                message: format!("Key press failed: {e}"),
            })?;
        Ok(())
    }

    async fn key_release(&self, key: &str) -> Result<(), CoreError> {
        use enigo::Keyboard;
        debug!(key, "[Enigo] key");
        let mut enigo = self.enigo.lock().await;
        enigo
            .key(Self::parse_key(key)?, enigo::Direction::Release)
            .map_err(|e| CoreError::Internal {
                code: maekon_core::error_codes::InternalCode::Generic,
                message: format!("Key release failed: {e}"),
            })?;
        Ok(())
    }

    async fn hotkey(&self, keys: &[String]) -> Result<(), CoreError> {
        use enigo::Keyboard;
        debug!(?keys, "[Enigo] key execution");
        let mut enigo = self.enigo.lock().await;
        for key_str in keys {
            enigo
                .key(Self::parse_key(key_str)?, enigo::Direction::Press)
                .map_err(|e| CoreError::Internal {
                    code: maekon_core::error_codes::InternalCode::Generic,
                    message: format!("Hotkey press failed: {e}"),
                })?;
        }
        for key_str in keys.iter().rev() {
            enigo
                .key(Self::parse_key(key_str)?, enigo::Direction::Release)
                .map_err(|e| CoreError::Internal {
                    code: maekon_core::error_codes::InternalCode::Generic,
                    message: format!("Hotkey release failed: {e}"),
                })?;
        }
        Ok(())
    }

    fn platform(&self) -> &str {
        #[cfg(target_os = "macos")]
        {
            "macos"
        }
        #[cfg(target_os = "windows")]
        {
            "windows"
        }
        #[cfg(target_os = "linux")]
        {
            "linux"
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            "unknown"
        }
    }
}

pub fn parse_mouse_button(button: &str) -> Result<MouseButton, CoreError> {
    MouseButton::parse_wire(button).map_err(|message| CoreError::InvalidArguments {
        code: maekon_core::error_codes::ValidationCode::InvalidArguments,
        message,
    })
}

pub fn create_platform_input_driver() -> Box<dyn InputDriver> {
    #[cfg(feature = "enigo")]
    {
        match EnigoInputDriver::new() {
            Ok(driver) => {
                tracing::info!("(enigo) initialize completed");
                return Box::new(driver);
            }
            Err(e) => {
                tracing::warn!("enigo initialize failure, NoOp: {e}");
            }
        }
    }
    Box::new(NoOpInputDriver)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn noop_driver_all_methods_ok() {
        // NoOpInputDriver is a pure no-op stub: every method returns Ok(()) with no side
        // effects. Ok(()) is the entire contract — there is no return value to inspect
        // beyond success. Each call is asserted individually so a future regression in
        // one method does not hide failures in the others. (#5594)
        let driver = NoOpInputDriver;
        driver
            .mouse_move(100, 200)
            .await
            .expect("NoOpInputDriver::mouse_move should always return Ok(())");
        driver
            .mouse_click("left", 100, 200)
            .await
            .expect("NoOpInputDriver::mouse_click should always return Ok(())");
        driver
            .type_text("hello")
            .await
            .expect("NoOpInputDriver::type_text should always return Ok(())");
        driver
            .key_press("Enter")
            .await
            .expect("NoOpInputDriver::key_press should always return Ok(())");
        driver
            .key_release("Enter")
            .await
            .expect("NoOpInputDriver::key_release should always return Ok(())");
        driver
            .hotkey(&["Ctrl".to_string(), "S".to_string()])
            .await
            .expect("NoOpInputDriver::hotkey should always return Ok(())");
    }

    #[test]
    fn noop_driver_platform() {
        let driver = NoOpInputDriver;
        assert_eq!(driver.platform(), "noop");
    }

    #[test]
    fn parse_mouse_button_variants() {
        assert_eq!(parse_mouse_button("left").expect("left"), MouseButton::Left);
        assert_eq!(parse_mouse_button("Left").expect("Left"), MouseButton::Left);
        assert_eq!(parse_mouse_button("l").expect("l"), MouseButton::Left);
        assert_eq!(
            parse_mouse_button("right").expect("right"),
            MouseButton::Right
        );
        assert_eq!(
            parse_mouse_button("Right").expect("Right"),
            MouseButton::Right
        );
        assert_eq!(parse_mouse_button("r").expect("r"), MouseButton::Right);
        assert_eq!(
            parse_mouse_button("middle").expect("middle"),
            MouseButton::Middle
        );
        assert_eq!(parse_mouse_button("m").expect("m"), MouseButton::Middle);
    }

    #[test]
    fn parse_mouse_button_rejects_unknown_and_empty_instead_of_left_click() {
        let unknown = parse_mouse_button("scrollwheel")
            .expect_err("unknown buttons must fail instead of becoming left clicks");
        assert!(
            matches!(unknown, CoreError::InvalidArguments { .. }),
            "unknown buttons must be rejected instead of becoming left clicks, got {unknown}"
        );

        let empty = parse_mouse_button("")
            .expect_err("empty button names must fail instead of left clicks");
        assert!(
            matches!(empty, CoreError::InvalidArguments { .. }),
            "empty button names must be rejected instead of becoming left clicks, got {empty}"
        );
    }

    #[test]
    fn factory_creates_driver() {
        let driver = create_platform_input_driver();
        let platform = driver.platform();
        assert!(!platform.is_empty());
    }

    #[cfg(feature = "enigo")]
    #[test]
    fn enigo_parse_key_special_keys() {
        assert!(matches!(
            EnigoInputDriver::parse_key("Enter"),
            Ok(enigo::Key::Return)
        ));
        assert!(matches!(
            EnigoInputDriver::parse_key("escape"),
            Ok(enigo::Key::Escape)
        ));
        assert!(matches!(
            EnigoInputDriver::parse_key("Ctrl"),
            Ok(enigo::Key::Control)
        ));
        assert!(matches!(
            EnigoInputDriver::parse_key("Command"),
            Ok(enigo::Key::Meta)
        ));
        assert!(matches!(
            EnigoInputDriver::parse_key("F1"),
            Ok(enigo::Key::F1)
        ));
    }

    #[cfg(feature = "enigo")]
    #[test]
    fn enigo_parse_key_unicode() {
        assert!(matches!(
            EnigoInputDriver::parse_key("a"),
            Ok(enigo::Key::Unicode('a'))
        ));
    }

    #[cfg(feature = "enigo")]
    #[test]
    fn enigo_parse_key_rejects_unknown_multichar_key_instead_of_synthesizing_a() {
        // Regression for oneshim#6124: an unknown multi-char key name must be rejected
        // (InvalidArguments) rather than silently mapped to Key::Unicode('a'), which
        // would inject a wrong/destructive keystroke. A known named key and a single
        // character still parse successfully.
        let err = EnigoInputDriver::parse_key("unknownkey")
            .expect_err("unknown multi-char key must be rejected");
        assert!(matches!(err, CoreError::InvalidArguments { .. }));
        assert!(matches!(
            EnigoInputDriver::parse_key("enter"),
            Ok(enigo::Key::Return)
        ));
        assert!(matches!(
            EnigoInputDriver::parse_key("x"),
            Ok(enigo::Key::Unicode('x'))
        ));
    }

    #[tokio::test]
    async fn noop_element_finder_returns_empty() {
        let finder = NoOpElementFinder;
        let result = finder.find_element(Some("test"), None, None).await.unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn noop_element_finder_name() {
        let finder = NoOpElementFinder;
        assert_eq!(finder.name(), "noop");
    }
}
