//! # oneshim-analysis
//!
//! LLM analysis pipeline — segment summarization, behavioral regime
//! classification, vector RAG retrieval, and proactive coaching engine.

// P2 nursery-hardening (PR-B): derive Eq alongside PartialEq when possible.
#![deny(clippy::derive_partial_eq_without_eq)]
// Cast safety: statistical values, scores, durations — precision loss acceptable.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]
// P2 PR-C: `missing_const_for_fn` accepted crate-wide. See
// docs/reviews/2026-04-21-p2-missing-const-for-fn-decision.md.
#![allow(clippy::missing_const_for_fn)]
// P2 remaining-nursery-lints: see decision doc.
#![allow(
    clippy::use_self,
    clippy::option_if_let_else,
    clippy::redundant_pub_crate
)]
// P2 PR-A (B2): `significant_drop_tightening` is accepted crate-wide.
// Rationale: this crate has 17 flagged sites across 7 files — all are either
// (a) tokio/std Mutex held across fast in-memory ops (clustering algorithms,
// regime classification) where contention cost is negligible, or (b) held
// across LLM/embedding calls where serialization is the intent. The nursery
// lint's false-positive rate on this crate is too high to enforce.
// See docs/reviews/2026-04-21-p2-significant-drop-tightening-spec.md §Category B.
#![allow(clippy::significant_drop_tightening)]

pub mod adaptive_search;
mod adaptive_trigger;
mod analyzer;
pub mod error;
pub mod fallback_analysis_provider;

mod assembler;
pub mod auto_tuner;
pub mod calibration_buffer;
pub mod clustering_strategy;
pub mod constraint_builder;
pub mod content_tracker;
pub mod daily_digest_generator;
pub mod daily_insight_generator;
pub mod digest_exporter;
pub mod document_heading;
pub mod embedding_pipeline;
pub mod focus_shared;
pub mod gmm_detector;
pub mod gui_aggregator;
pub mod gui_work_type_refiner;
pub mod hdbscan_detector;
#[cfg(feature = "hnsw")]
pub mod hnsw_adapter;
pub mod hybrid_search_service;
pub mod kmeans_adapter;
pub mod llm_segment_summarizer;
pub mod llm_work_type_refiner;
pub mod param_resolver;
mod pattern_miner;
mod prompts;
pub mod query_expander;
pub mod regime_analysis_facade;
pub mod regime_classifier;
pub mod regime_detector;
pub mod regime_manager;
pub mod segment_buffer;
mod segment_summarizer;
pub mod suggestion_filter;
pub mod terminal_detector;
mod title_bar_parser;
pub mod vector_retriever;
pub mod weekly_digest_generator;
mod work_type_classifier;

pub mod coaching_engine;
pub mod coaching_template;
pub mod feedback_tracker;
pub mod few_shot_selector;
pub mod regime_goal_tracker;

pub use adaptive_trigger::{AdaptiveTrigger, TriggerDecision};
pub use analyzer::ContextAnalyzer;
pub use assembler::{
    humanize_time_ago, AnalysisContext, ContentSummaryEntry, ContextAssembler, CurrentActivity,
    PiiFilter, RelevantHistoryEntry, SegmentStats, SessionMetrics,
};
pub use calibration_buffer::CalibrationBuffer;
pub use content_tracker::ContentTracker;
pub use few_shot_selector::FewShotSelector;
pub use param_resolver::ParamResolver;
pub use pattern_miner::{detect_gui_patterns, GuiPattern, PatternMiner};
pub use prompts::{FewShotExample, FewShotOutcome, PromptBuilder, ANALYSIS_SYSTEM_PROMPT};
pub use regime_classifier::RegimeClassifier;
pub use regime_detector::RegimeDetector;
pub use regime_manager::RegimeManager;
pub use segment_buffer::SegmentBuffer;
pub use segment_summarizer::{to_content_summary_entries, SegmentSummarizer};
pub use suggestion_filter::filter_by_regime;
pub use title_bar_parser::{ParsedContent, TitleBarParser};
pub use work_type_classifier::WorkTypeClassifier;

// Priority 2: Accuracy Improvements re-exports
pub use auto_tuner::{DriftDetector, EmaStatsTracker};
pub use clustering_strategy::{ClusterAssignment, ClusteringResult, ClusteringStrategy};
pub use regime_analysis_facade::RegimeAnalysisFacade;

pub use adaptive_search::{AdaptiveSearchCoordinator, SearchConfig, SearchStrategy};
pub use daily_digest_generator::DailyDigestGenerator;
pub use daily_insight_generator::DailyInsightGenerator;
pub use digest_exporter::DigestExporter;
pub use embedding_pipeline::EmbeddingPipeline;
pub use error::AnalysisError;
pub use gui_aggregator::GuiActivityAggregator;
pub use gui_work_type_refiner::GuiWorkTypeRefiner;
pub use hybrid_search_service::{HybridSearchService, SearchMode};
pub use llm_segment_summarizer::{LlmSegmentSummarizer, SEGMENT_SUMMARY_PROMPT};
pub use llm_work_type_refiner::LlmWorkTypeRefiner;
pub use query_expander::{ActivityContext, QueryExpander};
pub use vector_retriever::VectorRetriever;
pub use weekly_digest_generator::WeeklyDigestGenerator;

pub use coaching_engine::CoachingEngine;
pub use coaching_template::CoachingTemplateRegistry;
pub use fallback_analysis_provider::{FallbackAnalysisProvider, NoOpAnalysisProvider};
pub use feedback_tracker::FeedbackTracker;
pub use regime_goal_tracker::RegimeGoalTracker;

#[cfg(feature = "hnsw")]
pub use hnsw_adapter::HnswAdapter;
