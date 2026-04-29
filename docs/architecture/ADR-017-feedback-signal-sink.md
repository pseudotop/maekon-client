[English](./ADR-017-feedback-signal-sink.md) | [한국어](./ADR-017-feedback-signal-sink.ko.md)

# ADR-017: FeedbackSignalSink

**Status**: Accepted
**Date**: 2026-04-18
**Scope**: `oneshim-core::ports::feedback_signal_sink`, `oneshim-suggestion::FeedbackSender`, `oneshim-analysis::CoachingEngine/RegimeClassifier`, `src-tauri::feedback_sink::CompositeFeedbackSink`

---

## Context

Before this ADR, `commands/suggestions.rs::handle_suggestion_action` routed accept/reject/defer to `FeedbackSender::send_feedback`, which fired to the server via `ApiClient`. On failure it enqueued into `FeedbackRetryQueue`, which the scheduler drains. Nothing inside the client heard about those events — `CoachingEngine` never learned that a suggestion was accepted, `RegimeClassifier` never saw which regime's suggestions the user accepts vs rejects.

See the 2026-04-16 feature gap analysis (X3 remainder of C1).

## Decision

A new port `FeedbackSignalSink` in `oneshim-core::ports::feedback_signal_sink`:

```rust
#[async_trait]
pub trait FeedbackSignalSink: Send + Sync {
    async fn record_user_reaction(
        &self,
        feedback: &SuggestionFeedback,
    ) -> Result<(), CoreError>;
}
```

`CompositeFeedbackSink` in `src-tauri/src/feedback_sink/mod.rs` fans out to `Arc<CoachingEngine>` + `Arc<parking_lot::Mutex<RegimeClassifier>>` — each `Option<>`.

`FeedbackSender` gains an `Option<Arc<dyn FeedbackSignalSink>>`. `send_feedback` fires the sink BEFORE the API call so local learning adapts even when the server is unreachable. Existing `FeedbackSender::new(api)` is preserved as a shim calling `new_with_sink(api, None)`.

## Consequences

### Positive

- CoachingEngine + RegimeClassifier now have a stable channel for user-reaction signal. Concrete learning algorithm lands in a follow-up phase without touching the port.
- Fan-out is composition-root glue — no cross-crate adapter dependency.

### Negative / Constraints

- **Latency budget**: implementations MUST return within ~10 ms. Any blocking work (database writes, network calls, heavy computation) must be offloaded to `tokio::spawn` INSIDE the impl. `FeedbackSender::send_feedback` awaits the sink synchronously on the user-path accept/reject; breaking this budget re-introduces the write-path wait that was intentionally decoupled.
- **Err semantics**: `Result<(), CoreError>` is reserved for programmer bugs (mutex poisoning, invariant violations). All expected failure classes — network, database, transient unavailability — are the implementation's responsibility to log and swallow internally; they MUST NOT escalate as `Err`. The caller logs `warn!` on `Err` but does not treat it as a user-path failure.
- **Retry ordering**: `FeedbackSender::send_feedback` fires the sink before the API call on *every* invocation, including scheduler-driven retries when the network is down (`scheduler/loops/suggestions.rs` drains `FeedbackRetryQueue` by re-calling `accept` / `reject`). A single user action therefore produces N sink invocations for N retry attempts. This is an accepted hazard for Phase 3 because the current stubs (`CoachingEngine::record_user_reaction`, `RegimeClassifier::record_user_reaction`) are `debug!`-only and idempotent. **Any future learning impl MUST be idempotent per `suggestion_id` — dedupe at the impl layer (seen-set / last-seen timestamp), OR the follow-up phase hoists the sink call out of `send_feedback` and into `commands/suggestions.rs::handle_suggestion_action` so the sink fires once per user action, independent of network retries.** The addendum ADR that introduces the learning algorithm MUST pick one of these two options explicitly.

### Neutral

- `FeedbackSender::new_with_sink(api, None)` is always valid — telemetry-off / test / disabled-coaching paths all work unchanged.

## Alternatives considered

- `tokio::sync::broadcast` event bus — rejected. Adds a runtime task + sizing concern for two consumers and no per-event queuing need.
- Direct `Arc<CoachingEngine>` from `FeedbackSender` — rejected. Violates hexagonal boundary (`oneshim-suggestion` would depend on `oneshim-analysis`).
- One port per consumer (`CoachingSink`, `RegimeSink`) — rejected. Explodes port surface with no caller that wants to pick one-not-the-other; `CompositeFeedbackSink` handles `Option<>` per consumer.
- Fire the sink AFTER server call — rejected. Server failure would prevent local learning; local signal has independent value.

## References

- Implementation record: internal regime feedback learning spec and feature-gap analysis notes
- ADR-001 Hexagonal boundary
- ADR-007 `parking_lot::Mutex` never across `.await` — honoured by `CompositeFeedbackSink` (lock acquired, method called, lock dropped before any `.await`)
