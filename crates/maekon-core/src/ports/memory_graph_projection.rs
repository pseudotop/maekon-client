//! ADR-032 §2: the single shared projection seam between the ADR-023 memory
//! graph and any generation-adjacent consumer.
//!
//! Consumers (retrieval ranking in `maekon-web`, future Mode B/C consumers)
//! depend on THIS trait only — never on `MemoryGraphPort` directly and never
//! on the implementing crate (`maekon-analysis`). `src-tauri` wires the
//! implementation via DI (Port Instance Sharing, like `MemoryGraphPort`
//! through `WebServerRequiredDeps`).
//!
//! Mode separation is type-level: each approved ADR-032 mode gets its OWN
//! method with its OWN bounded return type. There is no mode enum, so
//! approving one method can never widen another mode's disclosure. Today only
//! Mode A (edge-topology retrieval ranking) is approved and present.

use crate::error::CoreError;
use crate::models::memory_graph::EdgeProjection;

/// Bounded, fail-closed memory-graph projection for generation-adjacent
/// consumers (ADR-032 §2).
///
/// # Fail-closed contract (ADR-032 §2, split semantics)
/// - An **unevaluable bound** — projection disabled, consent authority
///   unavailable or permission not granted, invalid window/floor/cap — yields
///   `Ok` with an **empty** [`EdgeProjection`], never an unbounded one.
/// - A **genuine storage failure** (`MemoryGraphPort` returning
///   `Err(CoreError)`) propagates as `Err` unchanged; masking it into empty
///   success would make "denied by policy" indistinguishable from "broken
///   storage" in contract tests.
#[async_trait::async_trait]
pub trait MemoryGraphProjectionPort: Send + Sync {
    /// Mode A: bounded edge-topology projection for in-process ranking.
    ///
    /// Reads claim rows only to resolve `Active` claim ids inside the
    /// configured window/floor/cap bounds; claim `text`/`kind`/`source` are
    /// never read. `now_secs` is epoch seconds and anchors the recency
    /// window (passed in so a caller's ranking pass is reproducible).
    ///
    /// # Errors
    /// `CoreError::Storage` for underlying SQLite failures (propagated, not
    /// masked — see the trait-level fail-closed contract).
    async fn project_edges_for_ranking(&self, now_secs: i64) -> Result<EdgeProjection, CoreError>;
}
