//! Port for accessing the coaching engine (goal tracking, nudges, feedback, snooze profiles).

use async_trait::async_trait;
use std::collections::HashMap;

use crate::models::coaching::GoalProgressView;

/// Port for accessing coaching engine capabilities from adapter crates.
///
/// Implemented by `CoachingEngine` in `maekon-analysis`.
/// Used by `maekon-web` REST handlers (blocking variants) and Tauri IPC
/// commands (async variants).
///
/// # Why synchronous `_blocking` methods? (ADR-001 section 2 deviation)
///
/// The Axum web handlers in `maekon-web` run inside a synchronous handler
/// context where `.await` is not directly available. The `_blocking` suffix
/// signals that these methods bridge the async `tokio::sync::RwLock` internals
/// via `tokio::task::block_in_place` + `Handle::block_on`.
///
/// This is a **deliberate deviation** from ADR-001 section 2 ("Apply `#[async_trait]`
/// to all port traits"). The coaching engine's internal state uses
/// `tokio::sync::RwLock` (not `std::sync`), making a fully sync-only trait
/// impossible. The hybrid approach (sync blocking wrappers + async methods)
/// serves both consumers:
///
/// - **Axum handlers** (`maekon-web`): call `_blocking` variants
/// - **Tauri IPC commands** (`src-tauri`): call async variants directly
///
/// # Async methods
///
/// The async methods (`snooze_profile`, `record_feedback`, `all_goal_progress`,
/// `update_regime_goals`) are used by Tauri IPC commands which already run in
/// an async context and can `.await` directly.
///
/// # Errors
/// **No fallible methods.** All methods return `Vec<GoalProgressView>`,
/// `Option<String>`, `u32`, or `()` — not `Result<_, _>`. Internal
/// failures (tokio RwLock poisoning, missing regime goals, unknown
/// profile) are logged by the `CoachingEngine` impl and return the
/// most sensible neutral value (empty Vec, None, 0, or silent no-op).
/// This matches the ADR-017 feedback-loop philosophy: coaching must
/// never block the UX path on its own errors.
#[async_trait]
pub trait CoachingPort: Send + Sync {
    /// Return goal progress for all configured regimes (blocking).
    fn all_goal_progress_blocking(&self) -> Vec<GoalProgressView>;

    /// Update the goal tracker's regime targets at runtime (blocking).
    fn update_regime_goals_blocking(&self, goals: &HashMap<String, u32>);

    /// Snooze a coaching profile for the given duration in seconds.
    ///
    /// While snoozed, `evaluate()` skips triggers for this profile.
    /// Called from the Tauri `dismiss_coaching_message` IPC command.
    async fn snooze_profile(&self, profile: &str, duration_secs: u64);

    /// Record explicit feedback (thumbs-up/down) for a coaching message.
    ///
    /// Called from the Tauri `submit_coaching_feedback` IPC command.
    async fn record_feedback(&self, message_id: &str, positive: bool);

    /// Return goal progress for all configured regimes (async).
    async fn all_goal_progress(&self) -> Vec<GoalProgressView>;

    /// Update the goal tracker's regime targets at runtime (async).
    async fn update_regime_goals(&self, goals: &HashMap<String, u32>);

    /// Return the label of the currently active regime (blocking).
    fn current_regime_label_blocking(&self) -> Option<String> {
        None
    }

    /// Return total minutes spent in regimes today (blocking).
    fn regime_minutes_today_blocking(&self) -> u32 {
        0
    }

    /// Return the label of the currently active regime (async).
    ///
    /// Default returns `None` — the same neutral value as the default
    /// `current_regime_label_blocking`. Implementors that hold async state
    /// (e.g. `CoachingEngine`, which wraps a `tokio::sync::RwLock`) MUST
    /// override this method with a genuine async implementation that reads
    /// the lock directly, without `block_in_place`.
    ///
    /// # Why this default does NOT delegate to `current_regime_label_blocking`
    ///
    /// The concrete `_blocking` override uses `block_in_place` + `block_on`
    /// to bridge an async lock onto a sync call-site. Calling that from an
    /// async default would route `block_in_place` back onto the async path,
    /// which panics on a current-thread runtime (e.g. `#[tokio::test]`).
    /// Returning the neutral value directly avoids the chain entirely while
    /// keeping the same observable behaviour for implementations that only
    /// override the `_blocking` variant. (Finding F-RR-C37-01 / #6007 §5)
    async fn current_regime_label(&self) -> Option<String> {
        None
    }

    /// Return total minutes spent in regimes today (async).
    ///
    /// Default returns `0` — the same neutral value as the default
    /// `regime_minutes_today_blocking`. Implementors that hold async state
    /// MUST override this method with a genuine async implementation.
    ///
    /// See `current_regime_label` for the rationale against delegating to
    /// the `_blocking` variant from an async default. (F-RR-C37-01 / #6007 §5)
    async fn regime_minutes_today(&self) -> u32 {
        0
    }

    /// Hot-reload the coaching config at runtime (#5707 hot-reload seam).
    ///
    /// Default implementation is a deliberate no-op so existing mock
    /// implementations in `maekon-web` handler tests continue to compile
    /// without change.  `CoachingEngine` overrides this in `port_impl.rs`
    /// to delegate to `CoachingEngine::update_config`.
    ///
    /// Called from:
    /// - `src-tauri/src/commands/settings.rs::update_setting` (Tauri IPC path)
    /// - `crates/maekon-web/src/handlers/settings.rs::update_settings` (REST path)
    async fn apply_config(&self, _config: crate::config::CoachingConfig) {}
}
