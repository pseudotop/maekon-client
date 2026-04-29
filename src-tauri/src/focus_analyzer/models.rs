// Re-export the canonical FocusStorage port from oneshim-core.
pub use oneshim_core::ports::focus_storage::FocusStorage;

// Re-export shared types so the rest of this crate can keep using
// `super::models::FocusAnalyzerConfig` etc. unchanged.
pub use oneshim_analysis::focus_shared::{
    CooldownType, FocusAnalyzerConfig, SessionTracker, SuggestionCooldowns,
};
