//! Suggestion → automation bridge: server-side run of the derived one-click
//! action (T4.1 #7917, ADR-027).
//!
//! The `#[tauri::command]` wrapper lives in `commands::automation` (the
//! overlay's only automation entry). This module holds the testable inner
//! logic: it re-derives the binding from the suggestion's OWN `(type, source)`
//! — the client never supplies a preset id — and routes execution through the
//! EXISTING `AutomationPort::run_workflow` gate chain. No new execution surface,
//! no gate change.

use std::sync::Arc;

use maekon_core::config::AutomationConfig;
use maekon_core::ports::automation::AutomationPort;
use maekon_storage::sqlite::SqliteStorage;

use crate::ipc_error::IpcError;
use crate::runtime_state::SuggestionRuntimeState;

use super::feedback::submit_suggestion_feedback_to_runtime;
use super::helpers::suggestion_not_found;
use super::mapping::{derive_bound_preset, parse_suggestion_source, parse_suggestion_type};

/// Run the automation action bound to `suggestion_id`, or return a clean
/// `IpcError` explaining why it could not run.
///
/// Flow (each step fails closed, emitting NOTHING on any non-success path so the
/// now-load-bearing scorer signal is never polluted):
/// 1. Reserve the id (reserve-then-execute) — refuse a concurrent double-fire.
/// 2. Look up the live suggestion (manager queue, else storage) — mirrors the
///    `pending_suggestions_snapshot` duality.
/// 3. Re-derive the preset from ITS `(type, source)`; this refuses non-RuleBased
///    / network / LLM origins and disabled automation, and resolves the preset
///    in the live builtins+custom list.
/// 4. `run_workflow` — the full existing gate chain (`ensure_enabled` →
///    confirmation policy → sandbox → per-step audit + storage hash-chain).
/// 5. ONLY on `success`: record the accept feedback (queue→history + scorer +
///    #7913 tally + best-effort server notify) and durably mark `acted_at`.
pub(crate) async fn run_suggestion_action_inner(
    suggestion_state: &SuggestionRuntimeState,
    storage: &SqliteStorage,
    automation: &AutomationConfig,
    controller: &Arc<dyn AutomationPort>,
    suggestion_id: &str,
) -> Result<(), IpcError> {
    // (1) Reserve-then-execute. The guard releases on every exit path (drop).
    let Some(_reservation) = suggestion_state.try_reserve_action(suggestion_id) else {
        return Err(IpcError::new(
            "validation.invalid_arguments",
            format!("An action is already running for suggestion: {suggestion_id}"),
        ));
    };

    // (2) Resolve the suggestion's own (type, source) — never trust client data.
    let (suggestion_type, source) =
        lookup_suggestion_type_source(suggestion_state, storage, suggestion_id).await?;

    // (3) Re-derive the binding server-side (refuses non-RuleBased/network/LLM,
    //     disabled automation, and dangling preset ids — all via one predicate).
    let Some(preset) = derive_bound_preset(&suggestion_type, &source, automation) else {
        return Err(IpcError::new(
            "validation.invalid_arguments",
            format!("Suggestion has no runnable automation action: {suggestion_id}"),
        ));
    };

    // (4) Execute through the EXISTING gate chain. Block/UserDenied surface as a
    //     CoreError here; a step failure surfaces as Ok(success=false). Neither
    //     emits any feedback.
    let result = controller
        .run_workflow(&preset)
        .await
        .map_err(IpcError::from)?;
    if !result.success {
        return Err(IpcError::new("internal.generic", result.message));
    }

    // (5) Success only. Record the accept feedback via the SAME sink the IPC
    //     accept path uses (queue→history move + scorer + tally + server notify).
    submit_suggestion_feedback_to_runtime(suggestion_state, storage, suggestion_id, "accept", None)
        .await?;

    // Durably mark acted_at. Best-effort: the manager path may hold a live-queue
    // suggestion that was never persisted (Ok(false)); the storage-fallback path
    // already marked it inside `submit_suggestion_feedback_to_runtime` (idempotent
    // re-mark here). Advisory, like the scorer tally write-through — never fail the
    // command after a successful run.
    if let Err(e) = storage.mark_unified_suggestion_acted(suggestion_id) {
        tracing::warn!(id = %suggestion_id, error = %e, "mark acted after run failed (advisory)");
    }

    Ok(())
}

/// Resolve `(SuggestionType, SuggestionSource)` for a live suggestion, mirroring
/// the `pending_suggestions_snapshot` duality: manager present ⇒ the live queue
/// is authoritative (a suggestion that has left the queue is no longer runnable);
/// manager absent ⇒ read from SQLite.
async fn lookup_suggestion_type_source(
    state: &SuggestionRuntimeState,
    storage: &SqliteStorage,
    suggestion_id: &str,
) -> Result<
    (
        maekon_core::models::suggestion::SuggestionType,
        maekon_core::models::suggestion::SuggestionSource,
    ),
    IpcError,
> {
    if let Some(mgr) = state.manager() {
        let found = {
            let queue = mgr.queue().lock().await;
            let found = queue
                .iter()
                .find(|s| s.suggestion_id == suggestion_id)
                .map(|s| (s.suggestion_type.clone(), s.source.clone()));
            found
        }; // queue lock dropped
        return found.ok_or_else(|| suggestion_not_found(suggestion_id));
    }

    // Storage fallback (no manager). A corrupt/unparseable row is treated as
    // "not found" rather than mis-mapped.
    let row = storage
        .list_suggestions(500)
        .map_err(IpcError::from)?
        .into_iter()
        .find(|row| row.suggestion_id == suggestion_id)
        .ok_or_else(|| suggestion_not_found(suggestion_id))?;
    let suggestion_type = parse_suggestion_type(&row.suggestion_type)
        .ok_or_else(|| suggestion_not_found(suggestion_id))?;
    let source =
        parse_suggestion_source(&row.source).ok_or_else(|| suggestion_not_found(suggestion_id))?;
    Ok((suggestion_type, source))
}
