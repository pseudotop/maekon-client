//! `Scheduler` required (VERIFIED-unconditional) dependencies (#7737 C1 PR-2).
//!
//! Split counterpart to the `with_*` builder setters still on
//! [`crate::scheduler::Scheduler`], which carry the genuinely optional/
//! runtime-conditional fields — 11 config-gated (`event_tx`/web.enabled,
//! `sync_engine`/sync.enabled, the RAG quartet/embedding,
//! `accessibility_extractor`/text_intelligence, `context_analyzer` +
//! `adaptive_trigger`/analysis.enabled, `batch_sink` + `api_client`/
//! offline_mode), 7 cfg-gated, and the a-caveat if-let-wrapped set
//! (`consent_manager`, `coaching_engine`, `magic_overlay`, etc. — always-Some
//! in production but deliberately kept `Option` this pass). This struct
//! carries the 8 fields that are unconditionally wired on EVERY shipped
//! deployment shape (default / `server` / `analysis` / `no-default-features`)
//! — never behind a runtime feature flag or config toggle.
//!
//! Each field here was individually re-verified against the wiring site in
//! `src-tauri/src/agent_runtime/mod.rs::AgentRuntimeBundle::run` at the time
//! of the split: either an unconditional `Arc::clone(&self.sqlite_storage_concrete)
//! as _` Port Instance Sharing cast, an unconditional concrete value built
//! inline (no `if let` / `match` / cfg / config-gate anywhere upstream), or
//! (`frame_storage`) pulled out of `Scheduler::new`'s former 9th positional
//! parameter, which the one prod call site always wrapped unconditionally in
//! `Some(...)`. Mirrors `crates/maekon-web/src/required_deps.rs` (#7738 D-2)
//! — same split rationale, same "no Default, no `..`" discipline.
//!
//! Deliberately: **no `Default` derive, no `..Default::default()` escape
//! hatch**. Every field must be named at the ONE construction site
//! (`agent_runtime/mod.rs::AgentRuntimeBundle::run`), so removing a field
//! from that literal — or forgetting to wire one when a new field is added —
//! is a compile error, not a silently-forgotten `None` (the #5703/#6264/#6266
//! wired-but-None incident class this split exists to remove).
//!
//! **CRITICAL shape**: `Scheduler`'s own struct fields for all 8 of these
//! names stay `Option<Arc<T>>` exactly as before this split — only this
//! payload type is non-Option. [`crate::scheduler::Scheduler::with_required_deps`]
//! destructures this struct and assigns `Some(value)` per field, so the 15+
//! loop/handler read sites doing `self.field.clone()` → `Option<Arc<T>>` stay
//! untouched (zero handler churn).
use std::sync::Arc;

use maekon_analysis::focus_analyzer::FocusAnalyzer;
use maekon_analysis::BeliefRevision;
use maekon_core::config_manager::ConfigManager;
use maekon_core::ports::calibration_store::CalibrationReader;
use maekon_core::ports::frame_storage::FrameStoragePort;
use maekon_core::ports::memory_graph_port::MemoryGraphPort;
use maekon_core::ports::regime_storage::RegimeStoragePort;

use crate::notification_manager::NotificationManager;

/// The 8-field VERIFIED-unconditional `Scheduler` dependency set. See the
/// module doc for the promotion criteria and the fields that deliberately
/// stay `Option` (set via the `with_*` builders still on
/// [`crate::scheduler::Scheduler`]).
pub struct SchedulerRequiredDeps {
    /// Pulled out of `Scheduler::new`'s former 9th positional parameter — the
    /// one prod call site (`agent_runtime/mod.rs::run`) always wrapped it
    /// `Some(support.frame_storage)`, and `support.frame_storage` itself is a
    /// non-Option `Arc<dyn FrameStoragePort>` on `AgentSupportContext`
    /// (`agent_runtime_support.rs`), built unconditionally by
    /// `AgentSupportContextBuilder::build` regardless of which internal
    /// branch (shared capture services vs. fresh construction) it takes.
    pub frame_storage: Arc<dyn FrameStoragePort>,
    /// `AgentSupportContext.notification_manager` — non-Option
    /// (`agent_runtime_support.rs:65`), built unconditionally by
    /// `AgentSupportContextBuilder::build` (no `if let` upstream).
    pub notification_manager: Arc<NotificationManager>,
    /// `AgentSupportContext.focus_analyzer` — non-Option
    /// (`agent_runtime_support.rs:66`), built unconditionally alongside
    /// `notification_manager`.
    pub focus_analyzer: Arc<FocusAnalyzer>,
    /// `AgentRuntimeBundle.config_manager` — a required (non-Option)
    /// constructor parameter of `AgentRuntimeBundle::new`, unconditionally
    /// forwarded to the scheduler.
    pub config_manager: ConfigManager,
    /// ADR-023: the same `SqliteStorage` Arc as `storage`/
    /// `sqlite_storage_concrete`, cast to `MemoryGraphPort` (Port Instance
    /// Sharing — no second handle). Lets the aggregation loop promote
    /// daily-digest content into durable claims + evidence edges.
    pub memory_graph: Arc<dyn MemoryGraphPort>,
    /// #7678 D4: the same `SqliteStorage` Arc, cast to `CalibrationReader`
    /// (Port Instance Sharing). Lets the aggregation loop's housekeeping
    /// block bound calibration_log growth via the configured retention
    /// window/row-cap.
    pub calibration_reader: Arc<dyn CalibrationReader>,
    /// ADR-023 Phase-2: LLM belief revision over the memory graph. Always
    /// constructed at the wiring site — the enrichment provider is
    /// LOCAL-LLM-gated internally (non-loopback endpoints degrade to
    /// `NoOpAnalysisProvider`), not by an `if let` at the wiring site itself.
    /// Consent + the `belief_revision_enabled` flag are still checked
    /// per-run by the aggregation loop.
    pub belief_revision: Arc<BeliefRevision>,
    /// #5810: the same underlying SQLite connection Arc, wrapped in
    /// `SqliteRegimeManagerStateStore` (the SAME wrapper the shutdown path
    /// uses) — periodic crash-durability checkpoint target for the
    /// aggregation loop. NOT to be confused with `regime_manager_arc`, an
    /// adjacent, similarly-named sibling field on `Scheduler` that stays
    /// `Option` — it is injected only `if let Some(rm) = self.regime_manager`
    /// at the wiring site, i.e. it is genuinely conditional and must NOT be
    /// promoted into this payload.
    pub regime_storage: Arc<dyn RegimeStoragePort>,
}
