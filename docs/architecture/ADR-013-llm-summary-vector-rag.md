# ADR-013: LLM Segment Summary + Vector RAG

| Field | Value |
|-------|-------|
| Status | Accepted |
| Date | 2026-03-18 |
| Scope | LlmSegmentSummarizer, EmbeddingProvider, VectorStore, EmbeddingPipeline, VectorRetriever, SemanticSearch, WeeklyDigest |

## Context

The adaptive tiered memory (ADR-012) segments desktop activity and produces rule-based stats. To complete the intelligence cycle, segments need LLM-generated natural language summaries and vector embeddings for semantic retrieval. This enables the LLM analysis pipeline to reference relevant historical context when generating suggestions, and users to search their activity history by meaning.

## Decisions

### §1 Two-Phase Segment Processing

On segment close:
- **Phase 1 (immediate)**: Rule-based stats saved, ContentActivity labels embedded and stored as vectors. Monitor loop not blocked.
- **Phase 2 (async)**: LLM summary generated via `AnalysisProvider::summarize_text()`, stored in `activity_segments`, then embedded as an additional vector.

Graceful degradation: LLM or embedding failure does not prevent segment storage. The segment is always persisted with at least rule-based stats.

### §2 AnalysisProvider Extension

Add `summarize_text()` default method to the existing `AnalysisProvider` port trait. Returns plain `String` instead of `Vec<Suggestion>`. Default implementation calls `analyze()` and extracts the first result's content. Adapters may override with a more efficient single-completion call.

### §3 New `oneshim-embedding` Crate

`fastembed-rs` (ONNX Runtime wrapper) is a heavy dependency (~30 MB dylib). To isolate it from `oneshim-network`:
- New crate `oneshim-embedding` depends only on `oneshim-core`
- Contains `LocalEmbeddingProvider` using `fastembed-rs`
- Feature-gated in `src-tauri`: `embedding = ["dep:oneshim-embedding"]`
- `fastembed::TextEmbedding::embed()` is synchronous — wrapped in `tokio::task::spawn_blocking`

Workspace grows from 11 to 12 crates.

### §4 EmbeddingProvider Port (async)

```rust
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, CoreError>;
    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, CoreError>;
    fn dimensions(&self) -> usize;
    fn model_id(&self) -> &str;
}
```

Two adapters: `LocalEmbeddingProvider` (fastembed-rs, in `oneshim-embedding`) and `RemoteEmbeddingProvider` (OpenAI API, in `oneshim-network`).

### §5 VectorStore Port with sqlite-vec Fallback

```rust
#[async_trait]
pub trait VectorStore: Send + Sync {
    async fn store(&self, vector: Vec<f32>, metadata: EmbeddingMetadata) -> Result<(), CoreError>;
    async fn search(&self, query: &[f32], limit: usize, time_decay_hours: f32) -> Result<Vec<SearchResult>, CoreError>;
    async fn enforce_retention(&self, max_days: u32) -> Result<u64, CoreError>;
    async fn mark_stale(&self, old_model_id: &str) -> Result<u64, CoreError>;
}
```

SQLite implementation:
- Primary: `sqlite-vec` extension for KNN search via `vec0` virtual table
- Fallback: brute-force cosine similarity in Rust over BLOB vectors (when sqlite-vec unavailable)
- `SqliteVectorStore.use_vec_extension: bool` detected at init time

Rowid synchronization: `embedding_vectors` metadata table and `embedding_index` virtual table linked by rowid within same transaction.

### §6 Time-Decayed Search

Combined score: `similarity × exp(-age_hours / decay_hours)`. Default decay: 168 hours (1 week half-life). Over-fetch 3x candidates from KNN, re-rank with time decay, return top-k.

### §7 Embedding Versioning

Each vector row stores `model_id`. On model change: mark stale → background re-embed from `original_text` (100 vectors/cycle, ~500ms). Stale vectors remain searchable until re-embedded.

### §8 Weekly Digest

Weekly rollup of segment data: regime/category breakdown, top content, deep work hours, context switches, comparison with previous week. Optional LLM narrative via `summarize_text()`. Generated Sunday midnight or on-demand.

### §9 PII Filtering

All text is PII-filtered BEFORE embedding. Uses the same injected `PiiFilter` closure from `oneshim-vision`. Embeddings encode semantics of filtered text only.

### §10 Privacy: Embedding Vectors as Behavioral Data

Embedding vectors encode semantic patterns of user activity. Even after PII filtering, vectors may reveal:
- Which projects/files the user works on
- Work timing patterns (when deep work sessions occur)
- Context switching behavior

Mitigations:
- All text is PII-filtered before embedding (§9), including `content_label` metadata
- Vectors are stored locally only (not transmitted unless server sync enabled)
- Consent gating via `activity_pattern_learning` permission (GDPR Tier 4)
- Retention policy (default 90 days) limits historical exposure
- Activity segments and weekly digests also have retention enforcement (90 days / 52 weeks)
- Users can delete all vectors via config reset or consent revocation

## Consequences

- Workspace grows from 11 to 12 crates (`oneshim-embedding`)
- `fastembed` + ONNX Runtime add ~30 MB external dependency (downloaded, not bundled)
- `sqlite-vec` extension adds ~1 MB (optional, with brute-force fallback)
- `AnalysisProvider` trait gains `summarize_text()` method (backward-compatible default impl)
- V10 migration adds `embedding_vectors`, `embedding_index`, `weekly_digests` tables
- ContextAssembler gains `relevant_history` parameter for RAG-enriched LLM context
- Two new API endpoints: semantic search + weekly digest

## References

- ADR-011: Standalone Analysis Pipeline
- ADR-012: Adaptive Tiered Memory
- Design spec: internal LLM summary/vector RAG design note
- Research: fastembed-rs, sqlite-vec, EWMA time decay
