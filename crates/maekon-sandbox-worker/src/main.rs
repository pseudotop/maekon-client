//! Worker binary that executes automation actions under platform sandbox constraints.
//!
//! The parent process applies platform-specific sandbox restrictions before spawning this binary.
//! It reads a SandboxRequest JSON from stdin, executes the action, then writes a
//! SandboxResponse JSON to stdout.

use enigo::{Button, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings};
use maekon_core::models::automation::{AutomationAction, MouseButton};
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
    /// Creates a success response for a completed action.
    fn ok() -> Self {
        Self {
            success: true,
            error: None,
        }
    }

    /// Creates a failure response carrying the reason for the failure.
    fn err(error: impl Into<String>) -> Self {
        Self {
            success: false,
            error: Some(error.into()),
        }
    }
}

fn main() {
    let response = match EnigoInputExecutor::new() {
        // Executor initialisation failure (e.g. no display) is reported safely via a stdout response.
        Err(e) => SandboxResponse::err(e),
        Ok(mut executor) => match read_request_line() {
            Err(e) => SandboxResponse::err(e),
            // Delegates a single line from stdin to the pure processing boundary (handle_request).
            Ok(line) => handle_request(&line, &mut executor),
        },
    };

    if let Ok(json) = serde_json::to_string(&response) {
        let _ = io::stdout().write_all(json.as_bytes());
        let _ = io::stdout().write_all(b"\n");
        let _ = io::stdout().flush();
    }
}

/// Reads one line of SandboxRequest JSON from stdin.
fn read_request_line() -> Result<String, String> {
    let stdin = io::stdin();
    stdin
        .lock()
        .lines()
        .next()
        .ok_or_else(|| "no input on stdin".to_string())?
        .map_err(|e| format!("stdin read error: {e}"))
}

/// Pure processing boundary of the stdin/stdout protocol (security isolation boundary).
///
/// Accepts one line of SandboxRequest JSON, executes the action, and returns a
/// SandboxResponse. Malformed JSON is handled safely as `success=false` + an error
/// message rather than panicking (defence against untrusted input at the isolation boundary).
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
            .button(parse_mouse_button(button)?, Direction::Click)
            .map_err(|error| format!("mouse click failed: {error}"))
    }

    fn key_type(&mut self, text: &str) -> Result<(), String> {
        self.enigo
            .text(text)
            .map_err(|error| format!("text input failed: {error}"))
    }

    fn key_press(&mut self, key: &str) -> Result<(), String> {
        self.enigo
            .key(parse_key(key)?, Direction::Press)
            .map_err(|error| format!("key press failed: {error}"))
    }

    fn key_release(&mut self, key: &str) -> Result<(), String> {
        self.enigo
            .key(parse_key(key)?, Direction::Release)
            .map_err(|error| format!("key release failed: {error}"))
    }
}

/// Maximum number of key units a single action may inject (KeyType characters / Hotkey keys).
/// Limits the blast radius of keystroke injection and the CPU cost of a single synthesis burst
/// (oneshim#5982). Requests that exceed this limit are rejected before reaching the OS.
const MAX_KEY_TYPE_CHARS: usize = 4096;

fn run_action(action: &AutomationAction, executor: &mut dyn InputExecutor) -> Result<(), String> {
    match action {
        AutomationAction::MouseMove { x, y } => executor.mouse_move(*x, *y),
        AutomationAction::MouseClick { button, x, y } => executor.mouse_click(button, *x, *y),
        AutomationAction::KeyType { text } => {
            let len = text.chars().count();
            if len > MAX_KEY_TYPE_CHARS {
                return Err(format!(
                    "key_type text too long: {len} chars (max {MAX_KEY_TYPE_CHARS})"
                ));
            }
            executor.key_type(text)
        }
        AutomationAction::KeyPress { key } => executor.key_press(key),
        AutomationAction::KeyRelease { key } => executor.key_release(key),
        AutomationAction::Hotkey { keys } => run_hotkey(executor, keys),
    }
}

/// Presses all keys in order, then releases **every** key that was actually pressed —
/// even if individual press/release calls fail along the way. This ensures that modifier
/// keys (Ctrl/Shift/Alt/Meta) can never be left stuck (oneshim#5983). Errors are
/// collected rather than causing an early return from the release loop, and are reported
/// together at the end.
fn run_hotkey(executor: &mut dyn InputExecutor, keys: &[String]) -> Result<(), String> {
    // oneshim#5982 blast-radius: cap how many keys a single Hotkey may inject,
    // reusing the same limit as KeyType. Rejected before any key reaches the OS.
    let count = keys.len();
    if count > MAX_KEY_TYPE_CHARS {
        return Err(format!(
            "hotkey too many keys: {count} keys (max {MAX_KEY_TYPE_CHARS})"
        ));
    }
    let mut pressed: Vec<&String> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    for key in keys {
        match executor.key_press(key) {
            Ok(()) => pressed.push(key),
            Err(e) => {
                errors.push(format!("press {key}: {e}"));
                break;
            }
        }
    }
    for key in pressed.iter().rev() {
        if let Err(e) = executor.key_release(key) {
            errors.push(format!("release {key}: {e}"));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!("hotkey failed: {}", errors.join("; ")))
    }
}

fn parse_mouse_button(button: &str) -> Result<Button, String> {
    Ok(match MouseButton::parse_wire(button)? {
        MouseButton::Left => Button::Left,
        MouseButton::Right => Button::Right,
        MouseButton::Middle => Button::Middle,
    })
}

fn parse_key(key: &str) -> Result<Key, String> {
    Ok(match key.to_lowercase().as_str() {
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
            // Only single characters are interpreted as Unicode key presses. Unknown
            // multi-character key names are rejected as an error rather than silently
            // synthesising 'a' (oneshim#5981) — prevents unintended key injection.
            let mut chars = other.chars();
            match (chars.next(), chars.next()) {
                (Some(ch), None) => Key::Unicode(ch),
                _ => return Err(format!("unknown key: {key}")),
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct MockInputExecutor {
        events: Vec<String>,
        /// Injects a key_release failure for the specified key (used to verify the
        /// stuck-modifier recovery path).
        fail_release_on: Option<String>,
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
            // Always records that a release was attempted (even on failure) — used to
            // assert the "every pressed key is released" invariant.
            self.events.push(format!("release:{key}"));
            if self.fail_release_on.as_deref() == Some(key) {
                return Err(format!("mock release failure: {key}"));
            }
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

        // press in order, release in reverse — orchestrated by run_hotkey.
        assert_eq!(
            executor.events,
            vec!["press:ctrl", "press:k", "release:k", "release:ctrl"]
        );
    }

    #[test]
    fn run_hotkey_releases_all_pressed_keys_even_when_a_release_fails() {
        // oneshim#5983: a mid-loop key_release failure must NOT leave an earlier
        // modifier stuck — every pressed key is still released, errors collected.
        let mut executor = MockInputExecutor {
            fail_release_on: Some("k".to_string()),
            ..Default::default()
        };
        let keys = vec!["ctrl".to_string(), "k".to_string()];

        let result = run_hotkey(&mut executor, &keys);

        // Verify the error message names the failing key so the caller can log it.
        let err = result.unwrap_err();
        assert!(
            err.contains("release k"),
            "expected release-k failure in: {err}"
        );
        // ctrl was STILL released after k's release failed (no stuck modifier).
        assert_eq!(
            executor.events,
            vec!["press:ctrl", "press:k", "release:k", "release:ctrl"]
        );
    }

    #[test]
    fn run_action_rejects_oversize_keytype() {
        // oneshim#5982: KeyType beyond MAX_KEY_TYPE_CHARS is rejected before the OS.
        let mut executor = MockInputExecutor::default();
        let action = AutomationAction::KeyType {
            text: "x".repeat(MAX_KEY_TYPE_CHARS + 1),
        };

        let result = run_action(&action, &mut executor);

        // Verify the error names the actual and max lengths so operators can diagnose it.
        let err = result.unwrap_err();
        assert!(
            err.contains("key_type text too long"),
            "expected size-limit error in: {err}"
        );
        assert!(executor.events.is_empty(), "executor must not be invoked");
    }

    #[test]
    fn run_action_rejects_oversize_hotkey() {
        // oneshim#5982: a Hotkey beyond MAX_KEY_TYPE_CHARS keys is rejected before
        // any key reaches the OS, sharing the KeyType blast-radius limit.
        let mut executor = MockInputExecutor::default();
        let action = AutomationAction::Hotkey {
            keys: vec!["a".to_string(); MAX_KEY_TYPE_CHARS + 1],
        };

        let result = run_action(&action, &mut executor);

        // Verify the error names the actual and max counts so operators can diagnose it.
        let err = result.unwrap_err();
        assert!(
            err.contains("hotkey too many keys"),
            "expected size-limit error in: {err}"
        );
        // No key was pressed or released — rejected before touching the executor.
        assert!(executor.events.is_empty(), "executor must not be invoked");
    }

    #[test]
    fn parse_key_supports_named_and_unicode_keys() {
        assert!(matches!(parse_key("enter"), Ok(Key::Return)));
        assert!(matches!(parse_key("x"), Ok(Key::Unicode('x'))));
    }

    #[test]
    fn parse_key_rejects_unknown_multichar_key_instead_of_synthesizing_a() {
        // oneshim#5981: an unrecognized multi-char key name must be an Err, not a
        // silent Key::Unicode('a') keypress.
        let err = parse_key("unknownkey").unwrap_err();
        assert!(err.contains("unknown key"), "unexpected error: {err}");
    }

    #[test]
    fn parse_mouse_button_supports_known_buttons() {
        assert!(matches!(parse_mouse_button("left"), Ok(Button::Left)));
        assert!(matches!(parse_mouse_button("l"), Ok(Button::Left)));
        assert!(matches!(parse_mouse_button("right"), Ok(Button::Right)));
        assert!(matches!(parse_mouse_button("R"), Ok(Button::Right)));
        assert!(matches!(parse_mouse_button("middle"), Ok(Button::Middle)));
        assert!(matches!(parse_mouse_button("m"), Ok(Button::Middle)));
    }

    #[test]
    fn parse_mouse_button_rejects_unknown_button_instead_of_synthesizing_left() {
        // oneshim#5981 (buttons): an unrecognized button name must be an Err, not a
        // silent Button::Left click — mirroring parse_key's reject-unknown contract.
        let err = parse_mouse_button("scrollwheel").unwrap_err();
        assert!(
            err.contains("unknown mouse button"),
            "unexpected error: {err}"
        );
    }

    /// Mock that forces every action to fail — used to verify the error response path.
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
    }

    // ── stdin/stdout protocol boundary (handle_request) tests — #4831 security isolation boundary ──

    #[test]
    fn handle_request_valid_json_dispatches_and_returns_success() {
        // Valid SandboxRequest JSON → actual action executed + success=true response.
        // AutomationAction uses externally-tagged serialisation, so the variant key is
        // nested under the "action" field.
        let line = r#"{"action":{"MouseClick":{"button":"left","x":10,"y":20}}}"#;
        let mut executor = MockInputExecutor::default();

        let response = handle_request(line, &mut executor);

        // The response must indicate success with no error field.
        assert!(response.success);
        assert!(response.error.is_none());
        // Also assert an observable side-effect showing the action actually reached the
        // mock executor (not merely theater).
        assert_eq!(executor.events, vec!["click:left,10,20"]);
    }

    #[test]
    fn handle_request_valid_keytype_reaches_executor() {
        // Confirms that another variant (KeyType) also passes through the boundary
        // and reaches the executor.
        let line = r#"{"action":{"KeyType":{"text":"hi"}}}"#;
        let mut executor = MockInputExecutor::default();

        let response = handle_request(line, &mut executor);

        assert!(response.success);
        assert_eq!(executor.events, vec!["type:2"]);
    }

    #[test]
    fn handle_request_malformed_json_returns_graceful_error_not_panic() {
        // Malformed JSON input → success=false + diagnostic message, no panic
        // (isolation boundary defence against untrusted input).
        let line = "this is not json {{{";
        let mut executor = MockInputExecutor::default();

        let response = handle_request(line, &mut executor);

        assert!(!response.success);
        let error = response
            .error
            .expect("malformed JSON must produce an error message");
        assert!(
            error.contains("invalid request JSON"),
            "unexpected error message: {error}"
        );
        // Rejected at the parse stage, so no action must have reached the executor.
        assert!(executor.events.is_empty());
    }

    #[test]
    fn handle_request_unknown_action_variant_returns_error_not_panic() {
        // Structurally valid JSON but with an unknown action variant → deserialisation
        // failure reported safely, no panic.
        let line = r#"{"action":{"SelfDestruct":{}}}"#;
        let mut executor = MockInputExecutor::default();

        let response = handle_request(line, &mut executor);

        assert!(!response.success);
        assert!(response.error.is_some());
        assert!(executor.events.is_empty());
    }

    #[test]
    fn handle_request_empty_input_returns_error_not_panic() {
        // An empty string must also produce an error response, not a panic.
        let mut executor = MockInputExecutor::default();

        let response = handle_request("", &mut executor);

        assert!(!response.success);
        assert!(response.error.is_some());
    }

    #[test]
    fn handle_request_action_execution_failure_propagates_to_error_response() {
        // A valid request where the executor fails must propagate the failure reason
        // into the response error field.
        let line = r#"{"action":{"MouseMove":{"x":1,"y":2}}}"#;
        let mut executor = FailingInputExecutor;

        let response = handle_request(line, &mut executor);

        assert!(!response.success);
        assert_eq!(response.error.as_deref(), Some("mock executor failure"));
    }
}
