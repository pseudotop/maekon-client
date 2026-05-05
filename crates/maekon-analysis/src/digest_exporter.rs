//! Markdown export for daily digests.
//!
//! Re-exports the canonical `DigestExporter` from `maekon_core` so that
//! analysis-layer code can reference it without reaching into core directly.

pub use maekon_core::models::daily_digest::DigestExporter;
