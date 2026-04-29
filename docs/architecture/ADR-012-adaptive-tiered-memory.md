# ADR-012: Adaptive Tiered Memory

| Field | Value |
|-------|-------|
| Status | Accepted |
| Date | 2026-03-18 |
| Scope | AdaptiveTrigger, CalibrationStore, RegimeDetector/Classifier/Manager, SegmentSummarizer, Content-Level Detection, WorkType Classification |

## Context

The analysis pipeline (ADR-011) generates suggestions but lacks temporal context — it analyzes point-in-time snapshots without understanding work segments or behavioral patterns over time. Fixed-interval summarization (hourly/daily) loses information: a 90-minute deep work session is split across boundaries, while a chaotic 10-minute period is diluted into a quiet hour.

We need adaptive segmentation driven by information density, with auto-discovered activity regimes that optimize parameters per work mode.

## Decisions

### §1 Adaptive Segmentation via Dual-EWMA Trigger

Segment boundaries are determined by a trigger scoring function, not wall-clock intervals. Four signals (density, importance, context change, buffer pressure) are combined with configurable weights. A hysteresis gate (T_high/T_low) prevents oscillation.

The AdaptiveTrigger is a **pure algorithm** in `oneshim-analysis` — no I/O dependencies. It receives `TriggerInput` events and emits segment lifecycle actions.

Minimum segment duration: 120 seconds (prevents meaningless micro-segments).
Maximum segment duration: 600 seconds (hard backstop forces summarization).

### §2 CalibrationStore — Buffered Sync Write + Async Read

Two port traits in `oneshim-core`:
- `CalibrationWriter: Send + Sync` — **synchronous** batch writes via `CalibrationBuffer` (flush every 10 entries or 5 seconds). Not per-event sync to avoid hot-path latency.
- `CalibrationReader: Send + Sync + #[async_trait]` — async bulk reads for regime detection.

All trigger inputs are persisted for retroactive recalibration, noise exclusion, and regime re-learning. Retention: 30 days OR 500,000 rows (ring buffer backstop).

Parameter snapshots are normalized: `params_version_id` references a separate `trigger_params_snapshots` table to avoid per-row JSON bloat.

### §3 Auto-Discovered Regimes

Regimes (activity modes) are **auto-discovered** via clustering, not manually defined. Preset profiles (Developer, Manager, etc.) serve as Day 1 seeds that get superseded by learned regimes.

**RegimeDetector**: hand-rolled k-means clustering (no external dependency). 5 features, max 7 clusters, silhouette score for optimal k selection. Runs daily or on-demand.

**RegimeClassifier**: real-time nearest-centroid matching on a 5-minute sliding window. Switches AdaptiveTrigger parameters on regime transition.

**RegimeManager**: lifecycle rules — creation (≥50 samples), merge (similar centroids), deactivation (14 days absent), archival (30 days inactive), limit (max 7 active).

### §4 Parameter Hierarchy (CSS Cascade)

Parameters resolve with specificity-based override:

```
Level 0: Global defaults (ResolvedParams::default())
Level 1: Regime overrides (Option fields — only Some values override)
Level 2: Category overrides (per AppCategory)
Level 3: Process overrides (per app name)
```

`TriggerParams` uses `Option<f32>` fields for the cascade model. `ResolvedParams` is the fully resolved (no Options) output. Weights are auto-normalized to sum to 1.0.

### §5 Content-Level Activity Detection

OCR-based detection of **what content** the user works on within each app, universally — not just for RDP/VM containers.

**TitleBarParser**: configurable per-app regex rules extracting content from window titles. Known patterns for VSCode, Chrome, Slack, Terminal, IntelliJ, Figma, etc.

**Container detection**: preset list of RDP/VM/VNC/Citrix apps. When active app is a container, OCR parses the inner title bar for sub-process detection.

**ContentTracker**: accumulates per-content durations within each segment.

### §6 WorkType Classification

Input activity patterns (keyboard/mouse from `InputActivityCollector`) are correlated with OCR content to classify **work type**: ActiveCoding, CodeReview, Writing, Reading, Designing, FormFilling, PassiveMeeting, etc.

**WorkTypeClassifier**: pure algorithm in `oneshim-analysis`. Takes `(KeyboardActivity, MouseActivity, content_label, app_category)` → `WorkType`.

WorkType transitions are significant events for the AdaptiveTrigger.

### §7 Consent & Privacy

CalibrationStore requires explicit consent via `ConsentManager` (`activity_pattern_learning` permission). Both `TieredMemoryConfig.enabled` and consent must be true.

### §8 Noise Handling

- Short anomalies (<1 hour, no regime match): flagged as noise, excluded from learning
- Sustained shifts (>24 hours): trigger regime re-detection
- Retroactive recalibration: user flags time range as noise → regime params recomputed
- All stored inputs enable rollback and re-learning

### §9 Integration with ContextAnalyzer

AdaptiveTrigger and ContextAnalyzer coexist:
- AdaptiveTrigger: **when** to segment (signal-based)
- ContextAnalyzer: **what** to suggest (LLM-based)

Integration: current segment stats + regime info feed into ContextAssembler for richer LLM context. Regime-aware suggestion filtering (suppress low-priority during Deep Focus, boost collaboration during Communication).

## Consequences

- `oneshim-analysis` grows with: AdaptiveTrigger, SegmentBuffer, CalibrationBuffer, RegimeDetector, RegimeClassifier, RegimeManager, SegmentSummarizer, TitleBarParser, ContentTracker, WorkTypeClassifier
- `oneshim-core` gains: TriggerParams/ResolvedParams, TriggerInput, CalibrationEntry, RegimeFeatures, Regime, SegmentSummary, ContentActivity, WorkType, EngagementMetrics, CalibrationWriter/CalibrationReader ports, TieredMemoryConfig, PresetProfile
- SQLite V9 migration adds 4 tables: calibration_log, trigger_params_snapshots, regimes, activity_segments
- No external ML dependencies (k-means hand-rolled)
- ContextAssembler gains `current_segment` + `current_regime` parameters
- Monitor loop feeds events to AdaptiveTrigger in addition to existing paths

## References

- ADR-011: Standalone Analysis Pipeline (foundation)
- ADR-001 §1-7: Error types, async traits, DI, crate boundaries
- ADR-003: Directory module pattern (apply when files exceed 500 lines)
- Design spec: internal adaptive tiered memory design note
- Research: Dual-EWMA, ESPRESSO, CUSUM, MemGPT memory consolidation
