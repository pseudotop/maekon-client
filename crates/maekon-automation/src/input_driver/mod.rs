use async_trait::async_trait;
use tracing::debug;

use maekon_core::error::CoreError;
use maekon_core::models::automation::MouseButton;
use maekon_core::models::intent::{ElementBounds, UiElement};
use maekon_core::ports::element_finder::ElementFinder;
use maekon_core::ports::input_driver::InputDriver;

#[cfg(feature = "enigo")]
mod activation;
#[cfg(feature = "enigo")]
mod enigo_driver;
mod trusted_paths;

#[cfg(feature = "enigo")]
pub use enigo_driver::EnigoInputDriver;

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
    #[cfg(all(
        feature = "enigo",
        any(target_os = "macos", target_os = "linux", target_os = "windows")
    ))]
    use super::activation::{run_activation_command, ActivationRun};
    #[cfg(all(target_os = "windows", feature = "enigo"))]
    use super::activation::{windows_is_maekon_host_alias, windows_is_maekon_main_window_title};
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    use super::trusted_paths::is_trusted_program;
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

    #[cfg(all(target_os = "windows", feature = "enigo"))]
    #[test]
    fn windows_maekon_alias_targets_only_the_exact_main_dashboard() {
        // Regression for #8466: the real Windows driver mapping must distinguish
        // the stable main dashboard from same-process Tracking/Overlay windows.
        assert!(windows_is_maekon_host_alias("Maekon"));
        assert!(windows_is_maekon_host_alias(" maekon "));
        assert!(!windows_is_maekon_host_alias("Visual Studio Code"));

        assert!(windows_is_maekon_main_window_title("Maekon"));
        assert!(!windows_is_maekon_main_window_title("Maekon Overlay"));
        assert!(!windows_is_maekon_main_window_title("Maekon Tracking"));
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

    #[test]
    fn window_activation_does_not_use_bare_name_path_lookup() {
        // #7075 regression: window-activation helpers must be spawned by a trusted
        // absolute path, never resolved through the inherited PATH (CWE-426/427).
        // Each forbidden pattern is built with format! so it does not appear
        // verbatim in this test source (which is part of the scanned files).
        // Mirrors sandbox::ipc::worker_path_resolution_does_not_use_path_lookup.
        //
        // ADR-003 split (#8691): the activation logic now spans this directory
        // module's sibling files, so every one of them is scanned instead of a
        // single `input_driver.rs`.
        let sources = [
            include_str!("mod.rs"),
            include_str!("enigo_driver.rs"),
            include_str!("activation.rs"),
            include_str!("trusted_paths.rs"),
        ];
        for program in ["open", "wmctrl", "xdotool", "powershell"] {
            let bare = format!("Command::new({program:?})");
            for source in sources {
                assert!(
                    !source.contains(&bare),
                    "{program} must be spawned by trusted absolute path, not bare name ({bare})"
                );
            }
        }
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn is_trusted_program_rejects_untrusted_paths() {
        use std::io::Write;
        // Relative paths are never trusted (must be absolute).
        assert!(
            !is_trusted_program(std::path::Path::new("open")),
            "relative paths must be rejected"
        );
        // A missing absolute path is rejected.
        assert!(
            !is_trusted_program(std::path::Path::new("/nonexistent/maekon/wmctrl")),
            "missing files must be rejected"
        );
        // A user-created, world-writable file is not trusted: it fails the
        // not-group/world-writable check (and the root-owned check when tests run
        // as non-root), so a planted binary in a writable dir is rejected.
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("wmctrl");
        {
            let mut f = std::fs::File::create(&file).expect("create temp helper");
            f.write_all(b"#!/bin/sh\n").expect("write");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o777))
                .expect("chmod 0777");
        }
        assert!(
            !is_trusted_program(&file),
            "world-writable / non-root-owned file must be rejected"
        );
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

    #[cfg(feature = "enigo")]
    #[tokio::test]
    async fn run_activation_command_reports_spawn_failure_for_missing_binary() {
        // A helper that cannot be spawned must report SpawnFailed (not error and
        // not hang) so the Linux path can fall through to the next helper and the
        // macOS/Windows paths can raise a precise "failed to spawn" error.
        let cmd = tokio::process::Command::new("/nonexistent/maekon/activation-helper");
        let outcome = run_activation_command(cmd, std::time::Duration::from_secs(5)).await;
        assert!(
            matches!(outcome, Ok(ActivationRun::SpawnFailed)),
            "a missing helper binary must yield SpawnFailed, got {outcome:?}"
        );
    }

    #[cfg(all(feature = "enigo", unix))]
    #[tokio::test]
    async fn run_activation_command_times_out_and_kills_hung_helper() {
        // #8055 P2-3: a helper that outlives the timeout must surface
        // ExecutionTimeout promptly (kill_on_drop reaps the child) rather than
        // hang, which would otherwise pin the per-suggestion action reservation
        // for the whole process lifetime.
        let mut cmd = tokio::process::Command::new("/bin/sleep");
        cmd.arg("30");
        let start = std::time::Instant::now();
        let outcome = run_activation_command(cmd, std::time::Duration::from_millis(150)).await;
        let elapsed = start.elapsed();
        assert!(
            matches!(
                outcome,
                Err(CoreError::ExecutionTimeout {
                    timeout_ms: 150,
                    ..
                })
            ),
            "a helper outliving the timeout must yield ExecutionTimeout(150ms), got {outcome:?}"
        );
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "must return at the timeout, not wait for the 30s helper (took {elapsed:?})"
        );
    }
}
