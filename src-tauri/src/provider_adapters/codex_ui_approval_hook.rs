//! Real Tauri-backed Codex `app-server` UI approval hook (E21 #5044, FU-A of
//! #4870).
//!
//! Replaces `DefaultDenyUiHook` (auto-decline) with a hook that:
//!   1. parks the decider's `oneshot::Sender<bool>` in a shared registry keyed
//!      by `request_id`, and
//!   2. emits a `codex:approval-request` Tauri event carrying the render-safe
//!      [`UiApprovalContext`] so the overlay `CodexApprovalModal` can render the
//!      command/file-diff context and let the user accept/decline/cancel.
//!
//! The eventual UI answer arrives via the `respond_codex_approval` Tauri command
//! (see `commands::ai_session`), which removes the parked sender from the SAME
//! registry and resolves it. accept→true, decline|cancel→false (cancel is a UI
//! verb mapping to a fail-closed decline; no new wire verdict).
//!
//! SINGLE TIMEOUT OWNER: this hook does NOT impose its own timeout — the decider
//! already owns `DEFAULT_UI_APPROVAL_TIMEOUT` (`codex_approval.rs`). On a decider
//! timeout the receiver is dropped, leaving a dead sender in the registry;
//! `respond_codex_approval` tolerates a missing/already-removed id. Adding a
//! second timeout here would create a double-decline race.
//!
//! FAIL-CLOSED on emit failure: if the event cannot be emitted (no overlay /
//! WebView yet), the parked sender simply never resolves and the decider's
//! timeout declines — the system stays fail-closed (never a silent accept).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use tauri::{AppHandle, Emitter};
use tokio::sync::oneshot;
use tokio::sync::Mutex;

use maekon_core::ports::codex_approval::{UiApprovalContext, UiApprovalHook};

/// The Tauri event name carrying a pending Codex approval to the overlay UI.
pub(crate) const CODEX_APPROVAL_EVENT: &str = "codex:approval-request";

/// Shared map of pending approval requests: a **hook-global** UI correlation id
/// → the decider's parked answer sender. The hook (writer) and the
/// `respond_codex_approval` command (reader/resolver) MUST share the SAME
/// instance — see [`crate::runtime_state::CodexApprovalRuntimeState`].
///
/// NOTE the key is the hook's OWN monotonic id, NOT the codex `request_id`: the
/// codex request id is per-session (each app-server process has an independent
/// counter), so two concurrent sessions sharing this one registry could collide
/// on it. The session answers the app-server with its own codex request id
/// separately (see `CodexAppServerSession::spawn_approval_task`), so this
/// re-keying never affects the server response — only the UI↔hook correlation.
pub(crate) type CodexApprovalRegistry = Arc<Mutex<HashMap<u64, oneshot::Sender<bool>>>>;

/// Real UI approval hook backed by a Tauri `AppHandle` + the shared registry.
pub(crate) struct CodexUiApprovalHook {
    app_handle: AppHandle,
    registry: CodexApprovalRegistry,
    /// Hook-global monotonic UI correlation id — collision-free across concurrent
    /// sessions (unlike the per-session codex `request_id`).
    next_ui_id: AtomicU64,
}

impl CodexUiApprovalHook {
    pub(crate) fn new(app_handle: AppHandle, registry: CodexApprovalRegistry) -> Self {
        Self {
            app_handle,
            registry,
            next_ui_id: AtomicU64::new(1),
        }
    }
}

#[async_trait]
impl UiApprovalHook for CodexUiApprovalHook {
    async fn request_approval(&self, ctx: UiApprovalContext) -> oneshot::Receiver<bool> {
        // Re-key on a hook-global monotonic id (the per-session codex request_id
        // is not unique across concurrent sessions sharing this registry). The
        // overlay echoes this id back via `respond_codex_approval`; the session
        // still answers the app-server with the original codex request id.
        let ui_id = self.next_ui_id.fetch_add(1, Ordering::Relaxed);
        let payload = UiApprovalContext {
            request_id: ui_id,
            ..ctx
        };
        let (tx, rx) = oneshot::channel();
        // Park the sender BEFORE emitting, so a (theoretically) instant UI answer
        // cannot arrive before the registry entry exists.
        self.registry.lock().await.insert(ui_id, tx);
        if let Err(e) = self.app_handle.emit(CODEX_APPROVAL_EVENT, &payload) {
            // Fail-closed: the decider's timeout will decline. Drop the parked
            // entry so it does not leak (the rx is still returned; it resolves to
            // Err on the decider side → decline).
            tracing::warn!(
                ui_request_id = ui_id,
                "failed to emit codex approval-request event (fail-closed): {e}"
            );
            self.registry.lock().await.remove(&ui_id);
        }
        rx
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The registry + oneshot fulfillment path the `respond_codex_approval`
    /// command relies on: a parked sender, removed by id and resolved `true`,
    /// makes the receiver resolve to `true`. (The emit path needs a real Tauri
    /// context, so it is exercised only in integration, not this unit test.)
    #[tokio::test]
    async fn parked_sender_resolves_receiver_on_remove_and_send() {
        let registry: CodexApprovalRegistry = Arc::new(Mutex::new(HashMap::new()));
        let (tx, rx) = oneshot::channel::<bool>();
        registry.lock().await.insert(42, tx);

        // Simulate respond_codex_approval(accept): remove + send true.
        let parked = registry.lock().await.remove(&42).expect("entry parked");
        parked.send(true).expect("receiver still alive");

        assert!(rx.await.expect("resolved"));
        assert!(
            registry.lock().await.is_empty(),
            "the entry is consumed on resolve"
        );
    }

    #[tokio::test]
    async fn missing_id_is_tolerated() {
        // respond_codex_approval for an unknown/already-resolved id is a no-op
        // (the decider has already timed out + declined). The registry remove
        // returns None — no panic, no double-resolve.
        let registry: CodexApprovalRegistry = Arc::new(Mutex::new(HashMap::new()));
        assert!(registry.lock().await.remove(&999).is_none());
    }
}
