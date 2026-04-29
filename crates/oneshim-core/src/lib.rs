// Cast safety: bounded metrics, coordinates, and IDs — precision loss acceptable.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]
// P2 PR-C: `missing_const_for_fn` accepted crate-wide. See
// docs/reviews/2026-04-21-p2-missing-const-for-fn-decision.md —
// const-viral cascade + nursery false-positive rate outweigh the value.
#![allow(clippy::missing_const_for_fn)]
// P2 remaining-nursery-lints: stylistic/cosmetic nursery lints accepted
// crate-wide. See docs/reviews/2026-04-21-p2-remaining-nursery-lints-decision.md.
#![allow(
    clippy::use_self,
    clippy::option_if_let_else,
    clippy::redundant_pub_crate
)]
// P2 nursery-hardening (PR-B): all PartialEq derives also derive Eq when
// possible. Float-carrying types use site-level #[allow] with reason.
#![deny(clippy::derive_partial_eq_without_eq)]

//! # oneshim-core

pub mod ai_model_lifecycle_policy;
pub mod app_registry;
pub mod binary_quantizer;
pub mod config;
pub mod config_manager;
pub mod consent;
pub mod error;
pub mod error_codes;
pub mod ivf_index;
pub mod models;
pub mod ports;
pub mod provider_surface;
pub mod quantization;
pub mod sanitized_display;
pub mod sync;
pub mod types;

pub use sanitized_display::{sanitized, SanitizedDisplay};

#[cfg(test)]
mod tests {
    use crate::models::suggestion::{Priority, Suggestion, SuggestionType};

    #[test]
    fn suggestion_serde_roundtrip() {
        let suggestion = Suggestion {
            suggestion_id: "sug_001".to_string(),
            suggestion_type: SuggestionType::WorkGuidance,
            content: "Commit your changes.".to_string(),
            priority: Priority::High,
            confidence_score: 0.95,
            relevance_score: 0.88,
            is_actionable: true,
            created_at: chrono::Utc::now(),
            expires_at: None,
            source: Default::default(),
            reasoning: None,
        };

        let json = serde_json::to_string(&suggestion).unwrap();
        let deserialized: Suggestion = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.suggestion_id, "sug_001");
        assert_eq!(deserialized.suggestion_type, SuggestionType::WorkGuidance);
        assert!(deserialized.confidence_score > 0.9);
    }

    #[test]
    fn priority_ordering() {
        assert!(Priority::Critical > Priority::High);
        assert!(Priority::High > Priority::Medium);
        assert!(Priority::Medium > Priority::Low);
    }

    #[test]
    fn config_defaults() {
        let config = crate::config::AppConfig::default_config();
        assert_eq!(config.monitor.poll_interval_ms, 1_000);
        assert_eq!(config.monitor.sync_interval_ms, 10_000);
        assert_eq!(config.storage.retention_days, 30);
        assert_eq!(config.storage.max_storage_mb, 500);
        assert_eq!(config.vision.thumbnail_width, 480);
        assert!(!config.vision.ocr_enabled);
    }
}
