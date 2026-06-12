//! #5032: crate-local alias for the workspace-wide circuit-breaker registry.
//!
//! The single shared `CircuitBreakerRegistry` (D7 / #4812 / E20-20) is threaded
//! as `Arc<…>` through many composition-root function signatures and builder
//! struct fields that themselves do NOT touch the network (the registry just
//! flows through them as an opaque handle; the only code that actually *consumes*
//! it by calling a `maekon-network` constructor — `AnalysisClient`,
//! `RemoteOcrProvider`, `RemoteLlmProvider`, `RemoteEmbeddingProvider`, … — is
//! already gated behind the `analysis`/`server` features).
//!
//! `maekon_network::CircuitBreakerRegistry` only exists when the `analysis`
//! feature pulls in `dep:maekon-network`. Referencing that path unconditionally
//! in those pass-through signatures is what broke `--no-default-features`
//! (E0433). This module re-exports the real type under `analysis` and provides a
//! byte-for-byte behaviour-neutral stand-in otherwise, so the pass-through
//! signatures compile under every feature-matrix cell.
//!
//! Behaviour is UNCHANGED whenever `analysis` is enabled (the default, `server`
//! and `grpc` builds), because the alias *is* the real type there. Under
//! `--no-default-features` the stub is constructed but never consumed by any
//! network code path (all such call sites are themselves `#[cfg]`-gated off), so
//! it is purely an inert placeholder satisfying the type system.

/// When `analysis` is on, the alias is exactly the upstream registry type — no
/// behaviour change.
#[cfg(feature = "analysis")]
pub(crate) use maekon_network::CircuitBreakerRegistry;

/// Inert stand-in used only when `maekon-network` is not compiled in
/// (`--no-default-features`). It is never passed to any network adapter — every
/// real consumer is gated behind `analysis`/`server` — so it carries no state
/// and does nothing.
#[cfg(not(feature = "analysis"))]
#[derive(Debug, Default)]
pub(crate) struct CircuitBreakerRegistry;

#[cfg(not(feature = "analysis"))]
impl CircuitBreakerRegistry {
    /// Mirror the upstream `CircuitBreakerRegistry::new()` constructor signature
    /// (returns `Arc<Self>`) so the existing pass-through call sites compile
    /// unchanged.
    pub(crate) fn new() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self)
    }
}

// NOTE: no unit test module here. The crate's test tree references
// `maekon_network` directly (e.g. `session_manager/tests.rs`), so it only
// compiles with the `analysis` feature on — under which this stub does not even
// exist (the alias is the real upstream type). The behavioural semantics are
// documented in the module-level doc-comment above; the stub's correctness is
// exercised by the `--no-default-features` build of every pass-through call site.
