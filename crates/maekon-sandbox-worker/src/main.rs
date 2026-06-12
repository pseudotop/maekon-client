//! 샌드박스 제약 아래에서 자동화 액션을 실행하는 worker 바이너리.
//!
//! 부모 프로세스가 플랫폼별 샌드박스 제약을 적용한 뒤 실행한다. stdin에서
//! SandboxRequest JSON을 읽고 액션을 실행한 뒤 SandboxResponse JSON을 stdout에 쓴다.

use enigo::{Button, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings};
use maekon_core::models::automation::AutomationAction;
use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, Write};

#[derive(Deserialize)]
struct SandboxRequest {
    action: AutomationAction,
}

#[derive(Serialize)]
struct SandboxResponse {
    success: bool,
    error: Option<String>,
}

impl SandboxResponse {
    /// 액션 실행 성공 응답을 생성한다.
    fn ok() -> Self {
        Self {
            success: true,
            error: None,
        }
    }

    /// 실패 사유를 담은 응답을 생성한다.
    fn err(error: impl Into<String>) -> Self {
        Self {
            success: false,
            error: Some(error.into()),
        }
    }
}

fn main() {
    let response = match EnigoInputExecutor::new() {
        // executor 초기화 실패(예: display 없음)도 stdout 응답으로 안전하게 보고한다.
        Err(e) => SandboxResponse::err(e),
        Ok(mut executor) => match read_request_line() {
            Err(e) => SandboxResponse::err(e),
            // stdin 의 한 줄을 순수 처리 경계(handle_request)로 위임한다.
            Ok(line) => handle_request(&line, &mut executor),
        },
    };

    if let Ok(json) = serde_json::to_string(&response) {
        let _ = io::stdout().write_all(json.as_bytes());
        let _ = io::stdout().write_all(b"\n");
        let _ = io::stdout().flush();
    }
}

/// stdin 에서 SandboxRequest JSON 한 줄을 읽는다.
fn read_request_line() -> Result<String, String> {
    let stdin = io::stdin();
    stdin
        .lock()
        .lines()
        .next()
        .ok_or_else(|| "no input on stdin".to_string())?
        .map_err(|e| format!("stdin read error: {e}"))
}

/// stdin/stdout 프로토콜의 순수 처리 경계 (보안 격리 boundary).
///
/// 한 줄의 SandboxRequest JSON 문자열을 받아 액션을 실행하고
/// SandboxResponse 를 반환한다. 잘못된 JSON 은 panic 하지 않고
/// `success=false` + error 메시지로 안전하게 처리한다 (격리 경계 신뢰 불가 입력 방어).
fn handle_request(line: &str, executor: &mut dyn InputExecutor) -> SandboxResponse {
    let request: SandboxRequest = match serde_json::from_str(line) {
        Ok(request) => request,
        Err(error) => return SandboxResponse::err(format!("invalid request JSON: {error}")),
    };

    match run_action(&request.action, executor) {
        Ok(()) => SandboxResponse::ok(),
        Err(error) => SandboxResponse::err(error),
    }
}

trait InputExecutor {
    fn mouse_move(&mut self, x: i32, y: i32) -> Result<(), String>;
    fn mouse_click(&mut self, button: &str, x: i32, y: i32) -> Result<(), String>;
    fn key_type(&mut self, text: &str) -> Result<(), String>;
    fn key_press(&mut self, key: &str) -> Result<(), String>;
    fn key_release(&mut self, key: &str) -> Result<(), String>;
    fn hotkey(&mut self, keys: &[String]) -> Result<(), String>;
}

struct EnigoInputExecutor {
    enigo: Enigo,
}

impl EnigoInputExecutor {
    fn new() -> Result<Self, String> {
        let enigo = Enigo::new(&Settings::default())
            .map_err(|error| format!("failed to initialize input executor: {error}"))?;
        Ok(Self { enigo })
    }
}

impl InputExecutor for EnigoInputExecutor {
    fn mouse_move(&mut self, x: i32, y: i32) -> Result<(), String> {
        self.enigo
            .move_mouse(x, y, Coordinate::Abs)
            .map_err(|error| format!("mouse move failed: {error}"))
    }

    fn mouse_click(&mut self, button: &str, x: i32, y: i32) -> Result<(), String> {
        self.mouse_move(x, y)?;
        self.enigo
            .button(parse_mouse_button(button), Direction::Click)
            .map_err(|error| format!("mouse click failed: {error}"))
    }

    fn key_type(&mut self, text: &str) -> Result<(), String> {
        self.enigo
            .text(text)
            .map_err(|error| format!("text input failed: {error}"))
    }

    fn key_press(&mut self, key: &str) -> Result<(), String> {
        self.enigo
            .key(parse_key(key), Direction::Press)
            .map_err(|error| format!("key press failed: {error}"))
    }

    fn key_release(&mut self, key: &str) -> Result<(), String> {
        self.enigo
            .key(parse_key(key), Direction::Release)
            .map_err(|error| format!("key release failed: {error}"))
    }

    fn hotkey(&mut self, keys: &[String]) -> Result<(), String> {
        for key in keys {
            self.key_press(key)?;
        }
        for key in keys.iter().rev() {
            self.key_release(key)?;
        }
        Ok(())
    }
}

fn run_action(action: &AutomationAction, executor: &mut dyn InputExecutor) -> Result<(), String> {
    match action {
        AutomationAction::MouseMove { x, y } => executor.mouse_move(*x, *y),
        AutomationAction::MouseClick { button, x, y } => executor.mouse_click(button, *x, *y),
        AutomationAction::KeyType { text } => executor.key_type(text),
        AutomationAction::KeyPress { key } => executor.key_press(key),
        AutomationAction::KeyRelease { key } => executor.key_release(key),
        AutomationAction::Hotkey { keys } => executor.hotkey(keys),
    }
}

fn parse_mouse_button(button: &str) -> Button {
    match button.to_lowercase().as_str() {
        "right" => Button::Right,
        "middle" => Button::Middle,
        _ => Button::Left,
    }
}

fn parse_key(key: &str) -> Key {
    match key.to_lowercase().as_str() {
        "enter" | "return" => Key::Return,
        "tab" => Key::Tab,
        "escape" | "esc" => Key::Escape,
        "backspace" => Key::Backspace,
        "delete" | "del" => Key::Delete,
        "space" => Key::Space,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        "up" | "uparrow" => Key::UpArrow,
        "down" | "downarrow" => Key::DownArrow,
        "left" | "leftarrow" => Key::LeftArrow,
        "right" | "rightarrow" => Key::RightArrow,
        "ctrl" | "control" => Key::Control,
        "shift" => Key::Shift,
        "alt" | "option" => Key::Alt,
        "meta" | "command" | "cmd" | "super" | "win" => Key::Meta,
        "capslock" => Key::CapsLock,
        "f1" => Key::F1,
        "f2" => Key::F2,
        "f3" => Key::F3,
        "f4" => Key::F4,
        "f5" => Key::F5,
        "f6" => Key::F6,
        "f7" => Key::F7,
        "f8" => Key::F8,
        "f9" => Key::F9,
        "f10" => Key::F10,
        "f11" => Key::F11,
        "f12" => Key::F12,
        other => {
            if let Some(ch) = other.chars().next() {
                if other.chars().count() == 1 {
                    return Key::Unicode(ch);
                }
            }
            Key::Unicode('a')
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct MockInputExecutor {
        events: Vec<String>,
    }

    impl InputExecutor for MockInputExecutor {
        fn mouse_move(&mut self, x: i32, y: i32) -> Result<(), String> {
            self.events.push(format!("move:{x},{y}"));
            Ok(())
        }

        fn mouse_click(&mut self, button: &str, x: i32, y: i32) -> Result<(), String> {
            self.events.push(format!("click:{button},{x},{y}"));
            Ok(())
        }

        fn key_type(&mut self, text: &str) -> Result<(), String> {
            self.events.push(format!("type:{}", text.len()));
            Ok(())
        }

        fn key_press(&mut self, key: &str) -> Result<(), String> {
            self.events.push(format!("press:{key}"));
            Ok(())
        }

        fn key_release(&mut self, key: &str) -> Result<(), String> {
            self.events.push(format!("release:{key}"));
            Ok(())
        }

        fn hotkey(&mut self, keys: &[String]) -> Result<(), String> {
            self.events.push(format!("hotkey:{}", keys.join("+")));
            Ok(())
        }
    }

    #[test]
    fn worker_action_runner_does_not_stub_actions() {
        let source = include_str!("main.rs");
        let stub_prefix = format!("sandbox-worker: {}", "mouse move");
        assert!(!source.contains(&stub_prefix));
    }

    #[test]
    fn run_action_dispatches_mouse_click_to_executor() {
        let mut executor = MockInputExecutor::default();
        let action = AutomationAction::MouseClick {
            button: "left".to_string(),
            x: 10,
            y: 20,
        };

        run_action(&action, &mut executor).unwrap();

        assert_eq!(executor.events, vec!["click:left,10,20"]);
    }

    #[test]
    fn run_action_dispatches_hotkey_to_executor() {
        let mut executor = MockInputExecutor::default();
        let action = AutomationAction::Hotkey {
            keys: vec!["ctrl".to_string(), "k".to_string()],
        };

        run_action(&action, &mut executor).unwrap();

        assert_eq!(executor.events, vec!["hotkey:ctrl+k"]);
    }

    #[test]
    fn parse_key_supports_named_and_unicode_keys() {
        assert!(matches!(parse_key("enter"), Key::Return));
        assert!(matches!(parse_key("x"), Key::Unicode('x')));
    }

    /// 액션 실행을 강제로 실패시켜 에러 응답 경로를 검증하기 위한 mock.
    #[derive(Default)]
    struct FailingInputExecutor;

    impl InputExecutor for FailingInputExecutor {
        fn mouse_move(&mut self, _x: i32, _y: i32) -> Result<(), String> {
            Err("mock executor failure".to_string())
        }
        fn mouse_click(&mut self, _button: &str, _x: i32, _y: i32) -> Result<(), String> {
            Err("mock executor failure".to_string())
        }
        fn key_type(&mut self, _text: &str) -> Result<(), String> {
            Err("mock executor failure".to_string())
        }
        fn key_press(&mut self, _key: &str) -> Result<(), String> {
            Err("mock executor failure".to_string())
        }
        fn key_release(&mut self, _key: &str) -> Result<(), String> {
            Err("mock executor failure".to_string())
        }
        fn hotkey(&mut self, _keys: &[String]) -> Result<(), String> {
            Err("mock executor failure".to_string())
        }
    }

    // ── stdin/stdout 프로토콜 경계 (handle_request) 테스트 — #4831 보안 격리 boundary ──

    #[test]
    fn handle_request_valid_json_dispatches_and_returns_success() {
        // 유효한 SandboxRequest JSON → 실제 액션 실행 + success=true 응답.
        // AutomationAction 은 externally-tagged 직렬화이므로 action 아래 variant 키로 중첩된다.
        let line = r#"{"action":{"MouseClick":{"button":"left","x":10,"y":20}}}"#;
        let mut executor = MockInputExecutor::default();

        let response = handle_request(line, &mut executor);

        // 응답이 성공이고 error 가 없어야 한다.
        assert!(response.success);
        assert!(response.error.is_none());
        // 그리고 실제로 mock executor 에 액션이 전달되었는지 관찰 가능한 효과를 단언한다 (theater 아님).
        assert_eq!(executor.events, vec!["click:left,10,20"]);
    }

    #[test]
    fn handle_request_valid_keytype_reaches_executor() {
        // 또 다른 variant(KeyType) 도 경계를 통과해 executor 에 도달하는지 확인.
        let line = r#"{"action":{"KeyType":{"text":"hi"}}}"#;
        let mut executor = MockInputExecutor::default();

        let response = handle_request(line, &mut executor);

        assert!(response.success);
        assert_eq!(executor.events, vec!["type:2"]);
    }

    #[test]
    fn handle_request_malformed_json_returns_graceful_error_not_panic() {
        // 깨진 JSON 입력 → panic 없이 success=false + 진단 메시지 응답 (격리 경계 방어).
        let line = "this is not json {{{";
        let mut executor = MockInputExecutor::default();

        let response = handle_request(line, &mut executor);

        assert!(!response.success);
        let error = response
            .error
            .expect("malformed JSON 은 error 메시지를 가져야 한다");
        assert!(
            error.contains("invalid request JSON"),
            "예상치 못한 에러 메시지: {error}"
        );
        // 파싱 단계에서 거부되었으므로 executor 에는 어떤 액션도 전달되지 않아야 한다.
        assert!(executor.events.is_empty());
    }

    #[test]
    fn handle_request_unknown_action_variant_returns_error_not_panic() {
        // 구조적으로는 JSON 이지만 알 수 없는 액션 variant → 역직렬화 실패를 안전하게 보고.
        let line = r#"{"action":{"SelfDestruct":{}}}"#;
        let mut executor = MockInputExecutor::default();

        let response = handle_request(line, &mut executor);

        assert!(!response.success);
        assert!(response.error.is_some());
        assert!(executor.events.is_empty());
    }

    #[test]
    fn handle_request_empty_input_returns_error_not_panic() {
        // 빈 문자열도 panic 없이 에러 응답으로 처리되어야 한다.
        let mut executor = MockInputExecutor::default();

        let response = handle_request("", &mut executor);

        assert!(!response.success);
        assert!(response.error.is_some());
    }

    #[test]
    fn handle_request_action_execution_failure_propagates_to_error_response() {
        // 유효한 요청이지만 executor 가 실행에 실패하면 그 사유가 응답 error 로 전파되어야 한다.
        let line = r#"{"action":{"MouseMove":{"x":1,"y":2}}}"#;
        let mut executor = FailingInputExecutor;

        let response = handle_request(line, &mut executor);

        assert!(!response.success);
        assert_eq!(response.error.as_deref(), Some("mock executor failure"));
    }
}
