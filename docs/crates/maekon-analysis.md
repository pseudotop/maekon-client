[English](./maekon-analysis.md) | [한국어](./maekon-analysis.ko.md)

# maekon-analysis

The standalone LLM analysis pipeline crate — segment summarization, behavioral
regime classification, vector RAG retrieval, adaptive clustering, coaching,
and (as of #7735 E-2) focus/workflow intelligence. An unconditional
dependency of `maekon-app` (the `hnsw` feature only gates the optional
`hnsw_adapter` module).

## Role

- **Context assembly + segmentation**: turns raw activity/frame telemetry into
  LLM-summarized segments, tracked over time.
- **Regime pipeline**: behavioral regime lifecycle (create, merge, split,
  mark_seen) with a facade for external consumption.
- **Vector search**: embedding pipeline (INT8 quantization), adaptive search
  strategy selection (brute-force / IVF / IVF+binary / HNSW), query expansion,
  few-shot example selection.
- **Clustering**: k-means, GMM, HDBSCAN detectors behind a common strategy
  trait.
- **Work classification**: rule-based + LLM-refined work-type classification,
  GUI activity aggregation, terminal/title-bar parsing.
- **Tuning + feedback**: drift detection, calibration buffering, feedback
  tracking, adaptive parameter resolution.
- **Digests + insights**: daily/weekly digest generation, daily insights,
  digest export.
- **Coaching**: proactive productivity coaching engine + template registry.
- **Focus + workflow intelligence** (#7735 E-2 arrival): app-usage relevance
  scoring, workflow-segment/playbook pattern detection, and the
  `FocusAnalyzer` rule-suggestion surface (break / focus-time /
  restore-context / pattern-detected).

## Directory Structure

```
maekon-analysis/src/
├── lib.rs                      # Crate root — mod decls + curated re-exports
├── analyzer.rs                 # ContextAnalyzer — LLM segment summarization + regime classification
├── assembler.rs                # ContextAssembler — analysis-context assembly + PII filtering
├── segment_buffer.rs, segment_summarizer.rs, llm_segment_summarizer.rs, content_tracker.rs
├── regime_classifier.rs, regime_detector.rs, regime_manager.rs,
│   regime_analysis_facade.rs, regime_goal_tracker.rs   # Behavioral regime pipeline
├── embedding_pipeline.rs       # EmbeddingPipeline — INT8 quantization
├── vector_retriever.rs, hybrid_search_service.rs, query_expander.rs, few_shot_selector.rs
├── adaptive_search/            # AdaptiveSearchCoordinator — auto strategy selection (directory module)
├── hnsw_adapter.rs             # HnswAdapter — `#[cfg(feature = "hnsw")]`
├── kmeans_adapter.rs, gmm_detector/, hdbscan_detector.rs, clustering_strategy.rs
├── work_type_classifier.rs, llm_work_type_refiner.rs, gui_work_type_refiner.rs,
│   gui_aggregator.rs, terminal_detector.rs, title_bar_parser/, document_heading.rs
├── auto_tuner.rs                # EmaStatsTracker + DriftDetector
├── adaptive_trigger/, calibration_buffer.rs, feedback_tracker.rs,
│   param_resolver.rs, constraint_builder.rs
├── daily_digest_generator.rs, weekly_digest_generator.rs,
│   daily_insight_generator.rs, digest_exporter.rs
├── coaching_engine/            # CoachingEngine — guards + triggers (directory module)
├── coaching_template/          # Template registry + built-in templates (directory module)
├── pattern_miner/               # Sequential/itemset GUI pattern mining (directory module)
├── focus_analyzer/              # FocusAnalyzer — session tracking + rule suggestions (#7735 E-2 arrival)
│   ├── mod.rs                  # FocusAnalyzer struct + on_app_switch_with_context/analyze_periodic/on_idle_resume
│   ├── models.rs                # Re-exports FocusAnalyzerConfig/CooldownType/SessionTracker/SuggestionCooldowns from focus_shared
│   └── suggestions.rs           # Break / focus-time / restore-context / pattern-detected rule suggestions
├── workflow_intelligence.rs     # WorkflowIntelligence — app-usage relevance scoring + playbook detection (#7735 E-2 arrival)
├── focus_shared.rs              # Shared FocusAnalyzer config/cooldown/tracker types + make_rule_suggestion
├── suggestion_filter.rs, prompts.rs, fallback_analysis_provider.rs
├── belief_revision.rs           # ADR-023 Phase-2 local LLM belief revision
└── error.rs                     # AnalysisError (ADR-019 typed codes)
```

## FocusAnalyzer / WorkflowIntelligence (#7735 E-2)

`focus_analyzer/` and `workflow_intelligence.rs` moved here from the
`src-tauri` composition root (#7735 extraction E-2) — both were already
tauri-free, ports-only domain logic. `FocusAnalyzer` depends only on
`maekon_core::ports::focus_storage::FocusStorage` and
`maekon_core::ports::notifier::DesktopNotifier`; `src-tauri` wires the
concrete adapters (`SqliteStorage`, `TauriNotifier`/`LogOnlyNotifier`) and
consumes the types via `maekon_analysis::focus_analyzer::FocusAnalyzer` and
`maekon_core::ports::focus_storage::FocusStorage` directly (no compat
re-export from `maekon_app`).

## Dependencies

- `maekon-core`: models, ports, errors (only normal-dependency edge)
- `thiserror`: `AnalysisError`
- `async-trait`: port trait implementations (e.g. `DesktopNotifier` in tests)
- `chrono`, `tokio` (`sync` feature), `tracing`, `lru`, `sha2`, `serde`/`serde_json`
- `hdbscan` (default feature), `usearch` (optional, `hnsw` feature)
- Dev-only: `maekon-storage` (cross-layer regression tests against real
  `SqliteStorage`), `tempfile`, `criterion`, `rand`, `maekon-core`
  `test-support` feature (`FakePiiSanitizer`)

## Build / Test

```bash
cargo check -p maekon-analysis
cargo test -p maekon-analysis
cargo test -p maekon-analysis --features hnsw
```
