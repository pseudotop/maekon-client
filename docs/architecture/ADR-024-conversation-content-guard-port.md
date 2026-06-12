[English](./ADR-024-conversation-content-guard-port.md) | [한국어](./ADR-024-conversation-content-guard-port.ko.md)

# ADR-024: Conversation Content Guard Port

**Status**: Accepted
**Date**: 2026-06-03
**Scope**: `src-tauri/src/provider_adapters/guarded_conversation.rs`, `src-tauri/src/provider_adapters/types.rs`, `src-tauri/src/session_manager/factory.rs`, `crates/maekon-core/src/ports/conversation_session.rs`
**Related**: ADR-001 (§6 dependency direction, §7 port placement), ADR-021 (consent core placement)
**Implementation**: `src-tauri/src/provider_adapters/guarded_conversation.rs`, `ConversationContentGuard` impl in `src-tauri/src/provider_adapters/types.rs`, `SessionManagerImpl::decorate_session` in `src-tauri/src/session_manager/factory.rs`

---

## Context

The conversation (chat) path transmitted user content to external providers (cloud HTTP APIs and provider-owned CLI subprocesses) **without any PII privacy guard**. The existing `GuardedLlmProvider`/`GuardedOcrProvider` decorators only sanitize the `LlmProvider`/`OcrProvider` traits (intent / screen-context / OCR), which are a different trait family from `ConversationSession`. Chat sessions flow through `ConversationSession::send_message`, were wrapped only by `AuditingSession` (which logs role metadata, not content), and reached the provider subprocess/HTTP transport as plaintext.

This gap (E21 review finding B1) is not specific to any in-flight migration; it existed in shipping code for every external chat session. Because Maekon is a desktop monitoring product, off-device transmission of unmasked user content is a material privacy defect.

Two design questions had to be settled before the fix:

1. **Where does the guard decorator live** — `maekon-core` (pure ports) or an adapter crate?
2. **How does the decorator depend on the guard** — on the broad concrete `ExternalOcrPrivacyGuard`, or on a narrow port abstraction?

## Decision

### 1. Add `is_external()` to the `ConversationSession` port

`ConversationSession` gains a default method `fn is_external(&self) -> bool { false }`, mirroring `LlmProvider::is_external`. Adapters that egress data off-device override it to `true`: `ClaudeSubprocessSession`, `GenericSubprocessSession` (Codex/Gemini CLI), and `HttpApiSession` (cloud — but **not** Ollama, a localhost endpoint). `LocalLlmSession` (Ollama) keeps the `false` default.

**Rationale**: the guard must distinguish on-device from off-device sessions so local backends are not needlessly gated. A defaulted method is backwards-compatible with all existing impls.

### 2. Introduce a narrow `ConversationContentGuard` port, src-tauri-local

```rust
#[async_trait]
pub(crate) trait ConversationContentGuard: Send + Sync {
    /// Returns Err (fail-closed) if safe transmission cannot be ensured.
    async fn sanitize_outbound(&self, message: &SessionMessage)
        -> Result<SessionMessage, CoreError>;
}
```

The port is defined in `src-tauri/src/provider_adapters/guarded_conversation.rs`, **not** in `maekon-core`. `ExternalOcrPrivacyGuard` implements it (reusing the same fail-closed consent / active-window / sensitive-app gate and `sanitize_title_with_level` filter as the screen/text LLM paths).

**Rationale**: per ADR-001 §7, a port trait consumed by more than one crate must live in `maekon-core`; a trait whose implementor *and* sole consumer both live in one adapter crate stays in that crate. Both `ExternalOcrPrivacyGuard` (impl) and `GuardedConversationSession` (consumer) live in `src-tauri`, so the port stays in `src-tauri` — this is **not** core pollution. The narrow single-method port (Interface Segregation) decouples the decorator from the broad OCR/LLM guard surface and makes the decorator's branch logic (passthrough / sanitize / fail-closed) trivially mockable, matching the `AuditingSession`→`AuditLogPort` precedent (the closest structural sibling — also a `ConversationSession` decorator depending on a port, not a concrete type).

### 3. `GuardedConversationSession` decorator in the adapter layer

```rust
async fn send_message(&self, message: &SessionMessage) -> Result<ResponseStream, CoreError> {
    if !self.inner.is_external() {
        return self.inner.send_message(message).await; // local: passthrough
    }
    let sanitized = self.guard.sanitize_outbound(message).await?; // fail-closed
    self.inner.send_message(&sanitized).await
}
```

The decorator self-gates on `inner.is_external()`. A guard error propagates via `?` **before** the inner session is called — fail-closed by construction.

### 4. Wire via `SessionManagerImpl` DI, audit-outermost

`SessionManagerImpl` gains an `Option<Arc<dyn ConversationContentGuard>>` field (builder `with_privacy_guard`). `factory.rs::decorate_session` wraps every freshly created session as `AuditingSession(GuardedConversationSession(inner))` — audit outermost, guard closest to the transport. The guard is constructed in `app_runtime_launch/session_wiring.rs` from an `ExternalOcrPrivacyGuard` (dedicated `ProcessTracker` for the active-window gate, shared session `AuditLogger` for the egress audit trail).

**Rationale**: fail-safe defaults (block on guard failure) and a single egress chokepoint match established AI-gateway / DLP practice.

## Consequences

### Positive

- Closes the chat-path PII gap (E21 B1): external chat content is sanitized before transmission; guard failure blocks transmission (fail-closed).
- Guard logic is unit-testable without spawning subprocesses (mock `ConversationContentGuard` for the decorator; constructed `ExternalOcrPrivacyGuard` + mock `ProcessMonitor` for the impl).
- Consistent with `AuditingSession`'s port-injection precedent; reuses the existing consent gate and PII filter — no parallel sanitization logic.
- Egress audit trail (`privacy.external_llm.allowed`) now covers the chat path.

### Negative

- One additional `Arc<dyn ConversationSession>` indirection layer per external session.
- A second guard-dependency style coexists in the codebase: LLM/OCR guards hold the concrete `ExternalOcrPrivacyGuard`, while the chat guard depends on the `ConversationContentGuard` port. This is intentional (see Alternatives) but is a divergence a future reader must understand.
- `session_wiring` constructs a dedicated `ProcessTracker` for the guard rather than sharing the runtime's monitor.

### Neutral

- Local (Ollama) sessions still flow through `decorate_session` but pass through the guard untouched via the `is_external()` gate.

## Alternatives Considered

**A. Decorator + port in `maekon-core`.** Rejected: `ExternalOcrPrivacyGuard` depends on `maekon_automation::AuditLogger` and `maekon_vision::PrivacyGateway`; hosting guard/decorator code in core would create reverse adapter→core dependencies (ADR-001 §6). The port also fails the ADR-001 §7 "consumed by >1 crate" bar, so core is the wrong home.

**B. Depend on the concrete `ExternalOcrPrivacyGuard` directly (no new port).** Matches the 3 existing LLM/OCR/Analysis guards and avoids a new trait. Rejected as the primary choice because: testing the decorator's three branches against the concrete guard requires constructing it with a mock `ProcessMonitor` and consent file (heavier), the decorator would couple to OCR/LLM-specific surface (ISP), and the closest sibling (`AuditingSession`) already depends on a port, not a concrete type. The narrow port is justified by present-day testability and the layer contract — not by hypothetical future implementations.

## Known Follow-ups

1. **Attachment-body sanitization** — `sanitize_outbound` currently masks `content` and free-text `context`. Text-like file attachments transmitted as base64 are not yet decoded/sanitized; track as a follow-up so raw attachment bodies are covered or stripped.
2. **Through-the-manager regression test** — full coverage of the create→guard→transmit chain is gated on subprocess spawning; add a fake-inner integration test that exercises `SessionManagerImpl` wiring without a real CLI.

## Related Docs

- `docs/architecture/ADR-001-rust-client-architecture-patterns.md` — §6 dependency direction, §7 port placement
- `docs/architecture/ADR-021-config-consent-core-placement.md` — consent/config core-placement boundary exception
