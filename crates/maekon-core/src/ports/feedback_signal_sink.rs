//! Feedback signal sink port.
//!
//! Cross-crate notification channel for user reactions to suggestions.
//! Implementations wrap `CoachingEngine`, `RegimeClassifier`, or any other
//! component that should adapt to accept/reject/defer signals.
//!
//! See `docs/architecture/ADR-017-feedback-signal-sink.md` for the full
//! rationale (latency budget, Err semantics, fan-out pattern).

use crate::error::CoreError;
use crate::models::suggestion::SuggestionFeedback;
use async_trait::async_trait;

/// Routes user reactions into learning components.
///
/// # Errors
/// `Err(_)` is reserved for **programmer bugs only** — mutex poisoning,
/// invariant violations, broken channel after spawn. The canonical
/// wire-code mapping for such faults is `CoreError::Internal` (wire:
/// `internal.generic`). Implementations MUST NOT escalate expected
/// failure classes (network, database, transient unavailability) as
/// `Err`; those are logged and swallowed internally per the ADR-017
/// fire-and-forget contract.
///
/// # Failure semantics
///
/// Fire-and-forget from the caller's perspective. `FeedbackSender` MUST NOT
/// block user-path accept/reject on a sink error.
///
/// # Latency budget
///
/// Implementations must return within ~10 ms. Any blocking work (database
/// writes, network calls, heavy computation) must be offloaded to
/// `tokio::spawn` INSIDE the impl so the inline path stays O(µs). The caller
/// awaits this future synchronously on the user-path accept/reject; breaking
/// this budget re-introduces the write-path wait we intentionally decoupled.
#[async_trait]
pub trait FeedbackSignalSink: Send + Sync {
    async fn record_user_reaction(&self, feedback: &SuggestionFeedback) -> Result<(), CoreError>;
}
