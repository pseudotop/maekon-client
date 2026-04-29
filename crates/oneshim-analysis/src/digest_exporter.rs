//! Markdown export for daily digests.
//!
//! Re-exports the canonical `DigestExporter` from `oneshim_core` so that
//! analysis-layer code can reference it without reaching into core directly.

pub use oneshim_core::models::daily_digest::DigestExporter;
