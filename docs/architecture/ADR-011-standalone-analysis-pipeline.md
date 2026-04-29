# ADR-011: Standalone Analysis Pipeline

| Field | Value |
|-------|-------|
| Status | Accepted |
| Date | 2026-03-18 |
| Scope | New `oneshim-analysis` crate, `AnalysisProvider` port, scheduler integration, Suggestion model unification |

## Context

The client collects rich desktop activity data (app switches, OCR text, window titles, focus metrics) and stores it in SQLite. Currently, suggestions are either rules-based (FocusAnalyzer) or server-dependent (SSE). We need a standalone analysis cycle that feeds collected context to an LLM and produces actionable suggestions without the connected server. The same logic must later be portable to the server's AI Intelligence domain.

This ADR covers five architectural decisions that existing ADRs do not address:

1. New adapter crate creation
2. Analysis-specific port contract
3. Orchestrator pattern for multi-port consumers
4. Scheduler loop extension
5. Suggestion model unification

## Decisions

### §1 New Adapter Crate: `oneshim-analysis`

A new workspace member `oneshim-analysis` is created under `crates/`.

**Dependency rules** (extends ADR-001 §6):
```
oneshim-core  ←  oneshim-analysis  (new)
              ←  oneshim-monitor
              ←  oneshim-vision
              ←  ...
oneshim-analysis  ←  src-tauri      (consumed by binary)
```

- MUST depend only on `oneshim-core` (port traits + domain models).
- MUST NOT depend on any other adapter crate (no `oneshim-network`, `oneshim-storage`, etc.).
- `src-tauri` wires concrete adapters into `oneshim-analysis` via DI.
- Error types use `thiserror` (library crate, per ADR-001 §1).
- Testing follows ADR-001 §5: manual mocks in `#[cfg(test)]` modules.

**Naming convention**: `oneshim-{domain}` where domain is a single word describing the crate's purpose.

**Crate structure** (follows ADR-003 if any file exceeds 500 lines):
```
crates/oneshim-analysis/
├── Cargo.toml
├── src/
│   ├── lib.rs              # pub re-exports
│   ├── analyzer.rs          # ContextAnalyzer orchestrator
│   ├── pattern_miner.rs     # pure algorithmic pattern detection
│   ├── assembler.rs         # context assembly + PII filtering
│   └── prompts.rs           # system prompt templates
```

### §2 AnalysisProvider Port Contract

A new port trait `AnalysisProvider` is defined in `oneshim-core/src/ports/analysis_provider.rs`.

```rust
#[async_trait]
pub trait AnalysisProvider: Send + Sync {
    async fn analyze(
        &self,
        context_json: &str,
        system_prompt: &str,
    ) -> Result<Vec<Suggestion>, CoreError>;

    fn provider_name(&self) -> &str;
}
```

**Design rationale**:
- Separate from existing `LlmProvider` trait (which returns `InterpretedAction` for UI automation).
- Returns `Vec<Suggestion>` directly — LLM response parsing is the adapter's responsibility, not the orchestrator's. Intermediate types (`SuggestionCandidate`) stay private within the adapter.
- Accepts raw `context_json` + `system_prompt` strings — the orchestrator controls prompt construction, the adapter handles HTTP transport.

**Error mapping**: LLM-specific failures (malformed response, content filter, token limit) use `CoreError::Analysis { code: ProviderCode::AnalysisFailed, message }` (wire: `provider.analysis_failed`) per [ADR-019](./ADR-019-error-code-infrastructure.md). Pre-ADR-019 the signature was `CoreError::Analysis(String)`.

**Implementation**: Lives in `oneshim-network/src/analysis_client.rs`, reusing the same HTTP client infrastructure as `RemoteLlmProvider`. Can implement both `LlmProvider` and `AnalysisProvider` on the same struct.

**Contract test** (per ADR-001 §5):
```rust
#[cfg(test)]
mod tests {
    struct MockAnalysisProvider { ... }

    #[async_trait]
    impl AnalysisProvider for MockAnalysisProvider {
        async fn analyze(&self, context: &str, prompt: &str)
            -> Result<Vec<Suggestion>, CoreError> { ... }
        fn provider_name(&self) -> &str { "mock" }
    }
}
```

### §3 Orchestrator Pattern

`ContextAnalyzer` is a **concrete struct** in `oneshim-analysis`, NOT a port trait. It consumes multiple ports to orchestrate the analysis cycle.

**Why not a port?**
- Ports represent single-responsibility I/O boundaries (ADR-001 §7).
- An orchestrator that internally calls `StorageService`, `PatternMiner`, `ContextAssembler`, and `AnalysisProvider` has multiple responsibilities.
- Making it a port would require all consumers to mock the entire orchestration surface.

**Pattern**:
```rust
pub struct ContextAnalyzer {
    storage: Arc<dyn StorageService>,
    analysis_provider: Arc<dyn AnalysisProvider>,
    pattern_miner: PatternMiner,      // owned, pure algorithm
    context_assembler: ContextAssembler, // owned, pure builder
    config: AnalysisConfig,
    last_analysis_at: Mutex<Option<DateTime<Utc>>>,
}
```

- Port dependencies injected via constructor (Arc<dyn T>, per ADR-001 §3).
- Pure algorithmic components (`PatternMiner`, `ContextAssembler`) are owned directly — they have no external I/O and don't need port abstraction.
- Interior mutability (`Mutex`) only for tracking throttle state (per ADR-001 §2).
- Constructed in `src-tauri/src/agent_runtime_support.rs` alongside other DI wiring.

**Precedent**: `AutomationController` in `oneshim-automation` follows the same pattern — concrete struct consuming multiple ports.

### §4 Scheduler Loop Extension

A new analysis loop is added to the scheduler as the 10th background loop.

**Integration point**: `src-tauri/src/scheduler/loops.rs` — new `spawn_analysis_loop()` method.

**Loop structure** (follows existing pattern from `spawn_focus_loop`, `spawn_sync_loop`):
```rust
pub(super) fn spawn_analysis_loop(
    &self,
    config: AnalysisConfig,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    let analyzer = self.context_analyzer.clone();
    let storage = self.storage.clone();

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(
            Duration::from_secs(config.interval_secs)
        );

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    match analyzer.analyze().await {
                        Ok(suggestions) => { /* store + notify */ }
                        Err(e) => warn!("analysis failure: {e}"),
                    }
                }
                _ = shutdown_rx.changed() => {
                    info!("analysis loop ended");
                    break;
                }
            }
        }
    })
}
```

**Naming convention**: `spawn_{name}_loop()`, matching existing pattern.

**Event-driven path**: Wired in `spawn_monitor_loop` alongside `FocusAnalyzer.on_app_switch_with_context()`. Both run in parallel on app switch; both output `Suggestion`.

**Deduplication**: If both FocusAnalyzer (rules) and ContextAnalyzer (LLM) produce suggestions for the same event, LLM-based takes priority (higher information density). Scheduler deduplicates before storing.

### §5 Suggestion Model Unification

`LocalSuggestion` is deprecated. All suggestions use the unified `Suggestion` model from `oneshim-core/src/models/suggestion.rs`.

**New field**: `source: SuggestionSource` with `#[serde(default)]` for backward compatibility with server SSE deserialization.

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SuggestionSource {
    #[default]
    RuleBased,
    LlmLocal,
    LlmServer,
}
```

**Migration**: `FocusAnalyzer` outputs `Suggestion` instead of `LocalSuggestion`. Existing `LocalSuggestion` enum variants map to `Suggestion` with appropriate `suggestion_type` and `content`.

**SQLite schema**: V8 migration creates unified `suggestions` table, replacing `local_suggestions`.

**Coexistence rule**: When server is active and returning suggestions via SSE, local LLM analysis (`LlmLocal`) is suppressed. Rules-based (`RuleBased`) always runs.

## Consequences

- Workspace grows from 10 to 11 crates.
- `oneshim-analysis` is fully testable in isolation (depends only on `oneshim-core` traits).
- `PatternMiner` and `ContextAssembler` are server-portable (no client-specific dependencies).
- Scheduler grows from 9 to 10 loops.
- `LocalSuggestion` is removed; all code paths use `Suggestion`.
- `AnalysisProvider` adapter in `oneshim-network` can be swapped for server-side DSPy pipeline without changing the orchestrator.

## References

- ADR-001 §1-7: Error types, async traits, DI, crate boundaries, ports
- ADR-003: Directory module pattern (apply if files exceed 500 lines)
- ADR-009: Client architecture baseline (runtime composition, delivery layer)
- Design spec: internal standalone analysis pipeline design note
