//! Read-only reader for the #4803 egress audit ledger (T1.2, #7910).
//!
//! Sibling of the append-only [`EgressLedgerSink`](crate::ports::egress_ledger)
//! writer: the sink records *what left the device*; this port reads those rows
//! back so the local "Trust Console" dashboard can render an egress
//! transparency browser ("what left this device"). The ledger is erase-retained
//! regulatory-compliance evidence, so this port is deliberately **read-only** —
//! it exposes no mutation surface.
//!
//! Implemented by `SqliteStorage` in `maekon-storage`, delegating to the
//! infallible inherent `recent_egress` / `egress_between` readers. Modeled as a
//! narrow standalone port (NOT folded into the much larger `WebStorage`
//! supertrait) for the same reason as `AuditChainVerifierPort` (#7600): adding
//! it must not ripple through every existing `WebStorage` manual mock across the
//! workspace. It is shared into the web layer as the same concrete
//! `SqliteStorage` `Arc`, cast to `Arc<dyn EgressLedgerReaderPort>` (Port
//! Instance Sharing), mirroring `memory_graph` / `audit_chain_verifier` /
//! `regime_storage`.

use crate::models::storage_records::EgressLedgerRecord;

/// Read-only view over the egress audit ledger.
///
/// **Synchronous + infallible**: a single local SQLite `SELECT` under a read
/// lock, no `.await`. Both methods mirror the inherent `SqliteStorage` readers,
/// which log `warn!` and return an empty `Vec` on any SQLite error rather than
/// surfacing a failure — a ledger read hiccup must never break the dashboard.
pub trait EgressLedgerReaderPort: Send + Sync {
    /// Return the most recent egress-ledger entries ordered by `occurred_at`
    /// descending, capped at `limit` rows.
    fn recent_egress(&self, limit: usize) -> Vec<EgressLedgerRecord>;

    /// Return egress-ledger entries whose `occurred_at` falls within the
    /// inclusive `[from, to]` RFC3339 range, ordered by `occurred_at` ascending.
    fn egress_between(&self, from: &str, to: &str) -> Vec<EgressLedgerRecord>;
}
