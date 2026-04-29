//! Productive-session detection + autostart counter increment.
//!
//! Per spec §5.5: Rust-side counter increment with idempotency via session_id.
//! No frontend round-trip — counter incremented in scheduler when ≥25 min focus
//! block completes. After increment, emits `autostart:eligible-for-prompt`
//! Tauri event if eligibility changed.
//!
//! Per Plan v2.5 Addendum A4: helper is generic over `R: Runtime` + uses an
//! inner closure-based fn for testability (no tauri::test runtime dependency).

use tauri::{AppHandle, Emitter, Runtime};
use tracing::warn;
use uuid::Uuid;

use oneshim_core::config::should_prompt;
use oneshim_core::config_manager::ConfigManager;
use oneshim_core::error_codes::AutostartCode;

const PRODUCTIVE_SESSION_THRESHOLD_SECS: u64 = 25 * 60;

/// Per-loop state for productive-session detection. Carried across monitor ticks.
/// Tracks the start time and unique ID of the current active focus block (if any).
#[derive(Default)]
pub struct FocusBlockState {
    start: Option<std::time::Instant>,
    id: Option<Uuid>,
}

impl FocusBlockState {
    /// Update on each idle tick. Detects idle↔active transitions using `idle_threshold`.
    /// Advances `prev_idle_secs` to `new_idle_secs` and calls
    /// `handle_focus_block_completed` when a block ends.
    pub fn tick<R: Runtime>(
        &mut self,
        prev_idle_secs: &mut u64,
        new_idle_secs: u64,
        idle_threshold: u64,
        app_handle: Option<&AppHandle<R>>,
        config_mgr: Option<&ConfigManager>,
    ) {
        let was_idle = *prev_idle_secs >= idle_threshold;
        let now_idle = new_idle_secs >= idle_threshold;
        if was_idle && !now_idle {
            // idle → active: start block
            if self.start.is_none() {
                self.start = Some(std::time::Instant::now());
                self.id = Some(Uuid::new_v4());
            }
        } else if !was_idle && now_idle {
            // active → idle: end block
            if let (Some(start), Some(id)) = (self.start.take(), self.id.take()) {
                let duration_secs = start.elapsed().as_secs();
                if let (Some(handle), Some(cm)) = (app_handle, config_mgr) {
                    handle_focus_block_completed(cm, handle, id, duration_secs);
                }
            }
        }
        *prev_idle_secs = new_idle_secs;
    }
}

/// Production entry point. Wraps the inner fn with a real Tauri event emit.
pub fn handle_focus_block_completed<R: Runtime>(
    config_mgr: &ConfigManager,
    app_handle: &AppHandle<R>,
    session_id: Uuid,
    duration_secs: u64,
) {
    handle_focus_block_completed_inner(
        config_mgr,
        || {
            if let Err(e) = app_handle.emit("autostart:eligible-for-prompt", ()) {
                warn!(
                    err.code = AutostartCode::EventEmitFailed.as_str(),
                    "failed to emit autostart:eligible-for-prompt: {e}"
                );
            }
        },
        session_id,
        duration_secs,
    );
}

/// Inner function — testable without Tauri runtime.
/// Takes a closure for event emission instead of an AppHandle.
pub(crate) fn handle_focus_block_completed_inner<F>(
    config_mgr: &ConfigManager,
    emit_event: F,
    session_id: Uuid,
    duration_secs: u64,
) where
    F: FnOnce(),
{
    if duration_secs < PRODUCTIVE_SESSION_THRESHOLD_SECS {
        return;
    }

    let session_id_str = session_id.to_string();
    let snapshot = match config_mgr.update_with(|c| {
        // Idempotency: if last_session_id matches, skip increment
        if c.autostart.last_session_id.as_deref() == Some(&session_id_str) {
            return Ok(());
        }
        c.autostart.productive_session_count =
            c.autostart.productive_session_count.saturating_add(1);
        c.autostart.last_session_id = Some(session_id_str.clone());
        Ok(())
    }) {
        Ok(s) => s,
        Err(e) => {
            warn!(
                err.code = AutostartCode::CounterIncrementFailed.as_str(),
                "{e}"
            );
            return;
        }
    };

    if should_prompt(&snapshot.autostart) {
        emit_event();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oneshim_core::config_manager::ConfigManager;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use tempfile::tempdir;

    fn make_config_mgr() -> (ConfigManager, tempfile::TempDir) {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("config.json");
        let mgr = ConfigManager::with_path(path).expect("ConfigManager::with_path");
        (mgr, dir)
    }

    #[test]
    fn short_block_does_not_increment() {
        let (mgr, _dir) = make_config_mgr();
        let called = Arc::new(AtomicBool::new(false));
        let called2 = called.clone();

        handle_focus_block_completed_inner(
            &mgr,
            move || {
                called2.store(true, Ordering::SeqCst);
            },
            Uuid::new_v4(),
            10 * 60, // 10 minutes — below threshold
        );

        let cfg = mgr.get();
        assert_eq!(cfg.autostart.productive_session_count, 0);
        assert!(!called.load(Ordering::SeqCst));
    }

    #[test]
    fn long_block_increments_counter() {
        let (mgr, _dir) = make_config_mgr();
        // Set state to Pending so should_prompt returns true after increment
        mgr.update_with(|c| {
            c.autostart.prompt_state = oneshim_core::config::AutostartPromptState::Pending;
            Ok(())
        })
        .expect("update");

        let called = Arc::new(AtomicBool::new(false));
        let called2 = called.clone();

        handle_focus_block_completed_inner(
            &mgr,
            move || {
                called2.store(true, Ordering::SeqCst);
            },
            Uuid::new_v4(),
            25 * 60, // exactly at threshold
        );

        let cfg = mgr.get();
        assert_eq!(cfg.autostart.productive_session_count, 1);
        assert!(called.load(Ordering::SeqCst));
    }

    #[test]
    fn idempotency_same_session_id_no_double_increment() {
        let (mgr, _dir) = make_config_mgr();
        let id = Uuid::new_v4();

        for _ in 0..3 {
            handle_focus_block_completed_inner(&mgr, || {}, id, 30 * 60);
        }

        let cfg = mgr.get();
        assert_eq!(
            cfg.autostart.productive_session_count, 1,
            "must not double-increment"
        );
    }

    #[test]
    fn different_session_ids_each_increment() {
        let (mgr, _dir) = make_config_mgr();

        for _ in 0..3 {
            handle_focus_block_completed_inner(&mgr, || {}, Uuid::new_v4(), 30 * 60);
        }

        let cfg = mgr.get();
        assert_eq!(cfg.autostart.productive_session_count, 3);
    }

    #[test]
    fn dismissed_state_skips_event_emission() {
        let (mgr, _dir) = make_config_mgr();
        // Set state to Dismissed — should_prompt() returns false regardless of count
        mgr.update_with(|c| {
            c.autostart.prompt_state = oneshim_core::config::AutostartPromptState::Dismissed;
            Ok(())
        })
        .expect("update");

        let called = Arc::new(AtomicBool::new(false));
        let called2 = called.clone();

        handle_focus_block_completed_inner(
            &mgr,
            move || {
                called2.store(true, Ordering::SeqCst);
            },
            Uuid::new_v4(),
            30 * 60, // long block — would normally trigger event
        );

        let cfg = mgr.get();
        // Counter still increments (idempotency tracking is separate from event emission)
        assert_eq!(cfg.autostart.productive_session_count, 1);
        // But event must NOT fire because prompt_state is Dismissed
        assert!(
            !called.load(Ordering::SeqCst),
            "event should NOT emit when prompt_state == Dismissed"
        );
    }
}
