use async_trait::async_trait;
use tracing::{debug, warn};

use maekon_core::error::CoreError;
use maekon_core::models::automation::MouseButton;
use maekon_core::ports::input_driver::InputDriver;

use super::activation::activate_app_platform;
use super::parse_mouse_button;

pub struct EnigoInputDriver {
    enigo: tokio::sync::Mutex<enigo::Enigo>,
}

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

    pub(super) fn parse_key(key: &str) -> Result<enigo::Key, CoreError> {
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

    async fn activate_app(&self, app_name: &str) -> Result<bool, CoreError> {
        // enigo synthesizes input only; window activation is a separate OS concern,
        // so this shells out to the platform's window manager (consistent with the
        // existing osascript/xdotool shell-outs in maekon-monitor). `app_name` is an
        // argv element (macOS/Linux) or read from an env var (Windows PowerShell) —
        // never interpolated into a shell/AppleScript string, so there is no
        // injection surface from untrusted preset/LLM/template-driven names. The
        // helper binaries are resolved to trusted absolute paths (not the inherited
        // PATH) so a planted binary cannot be executed (#7075).
        debug!(app_name, "[Enigo] activate app");
        activate_app_platform(app_name).await
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
