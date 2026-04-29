use async_trait::async_trait;
use tracing::debug;

use oneshim_core::error::CoreError;
use oneshim_core::models::intent::{ElementBounds, UiElement};
use oneshim_core::ports::element_finder::ElementFinder;
use oneshim_core::ports::input_driver::InputDriver;

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

    fn parse_key(key: &str) -> enigo::Key {
        match key.to_lowercase().as_str() {
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
                if let Some(ch) = other.chars().next() {
                    if other.chars().count() == 1 {
                        return enigo::Key::Unicode(ch);
                    }
                }
                debug!("unknown key: {other}, Unicode 'a'");
                enigo::Key::Unicode('a')
            }
        }
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
                code: oneshim_core::error_codes::InternalCode::Generic,
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
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: format!("Mouse move failed: {e}"),
            })?;
        let btn = match parse_mouse_button(button) {
            "right" => enigo::Button::Right,
            "middle" => enigo::Button::Middle,
            _ => enigo::Button::Left,
        };
        enigo
            .button(btn, enigo::Direction::Click)
            .map_err(|e| CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: format!("Mouse click failed: {e}"),
            })?;
        Ok(())
    }

    async fn type_text(&self, text: &str) -> Result<(), CoreError> {
        use enigo::Keyboard;
        debug!(text_len = text.len(), "[Enigo] text");
        let mut enigo = self.enigo.lock().await;
        enigo.text(text).map_err(|e| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("Text input failed: {e}"),
        })?;
        Ok(())
    }

    async fn key_press(&self, key: &str) -> Result<(), CoreError> {
        use enigo::Keyboard;
        debug!(key, "[Enigo] key");
        let mut enigo = self.enigo.lock().await;
        enigo
            .key(Self::parse_key(key), enigo::Direction::Press)
            .map_err(|e| CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: format!("Key press failed: {e}"),
            })?;
        Ok(())
    }

    async fn key_release(&self, key: &str) -> Result<(), CoreError> {
        use enigo::Keyboard;
        debug!(key, "[Enigo] key");
        let mut enigo = self.enigo.lock().await;
        enigo
            .key(Self::parse_key(key), enigo::Direction::Release)
            .map_err(|e| CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
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
                .key(Self::parse_key(key_str), enigo::Direction::Press)
                .map_err(|e| CoreError::Internal {
                    code: oneshim_core::error_codes::InternalCode::Generic,
                    message: format!("Hotkey press failed: {e}"),
                })?;
        }
        for key_str in keys.iter().rev() {
            enigo
                .key(Self::parse_key(key_str), enigo::Direction::Release)
                .map_err(|e| CoreError::Internal {
                    code: oneshim_core::error_codes::InternalCode::Generic,
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

pub fn parse_mouse_button(button: &str) -> &str {
    match button.to_lowercase().as_str() {
        "left" | "l" => "left",
        "right" | "r" => "right",
        "middle" | "m" => "middle",
        _ => "left", // default value
    }
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
        let driver = NoOpInputDriver;
        assert!(driver.mouse_move(100, 200).await.is_ok());
        assert!(driver.mouse_click("left", 100, 200).await.is_ok());
        assert!(driver.type_text("hello").await.is_ok());
        assert!(driver.key_press("Enter").await.is_ok());
        assert!(driver.key_release("Enter").await.is_ok());
        assert!(driver
            .hotkey(&["Ctrl".to_string(), "S".to_string()])
            .await
            .is_ok());
    }

    #[test]
    fn noop_driver_platform() {
        let driver = NoOpInputDriver;
        assert_eq!(driver.platform(), "noop");
    }

    #[test]
    fn parse_mouse_button_variants() {
        assert_eq!(parse_mouse_button("left"), "left");
        assert_eq!(parse_mouse_button("Left"), "left");
        assert_eq!(parse_mouse_button("l"), "left");
        assert_eq!(parse_mouse_button("right"), "right");
        assert_eq!(parse_mouse_button("Right"), "right");
        assert_eq!(parse_mouse_button("r"), "right");
        assert_eq!(parse_mouse_button("middle"), "middle");
        assert_eq!(parse_mouse_button("m"), "middle");
    }

    #[test]
    fn parse_mouse_button_default() {
        assert_eq!(parse_mouse_button("unknown"), "left");
        assert_eq!(parse_mouse_button(""), "left");
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
            enigo::Key::Return
        ));
        assert!(matches!(
            EnigoInputDriver::parse_key("escape"),
            enigo::Key::Escape
        ));
        assert!(matches!(
            EnigoInputDriver::parse_key("Ctrl"),
            enigo::Key::Control
        ));
        assert!(matches!(
            EnigoInputDriver::parse_key("Command"),
            enigo::Key::Meta
        ));
        assert!(matches!(EnigoInputDriver::parse_key("F1"), enigo::Key::F1));
    }

    #[cfg(feature = "enigo")]
    #[test]
    fn enigo_parse_key_unicode() {
        assert!(matches!(
            EnigoInputDriver::parse_key("a"),
            enigo::Key::Unicode('a')
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
