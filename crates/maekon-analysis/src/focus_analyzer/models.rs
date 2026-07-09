// Re-export shared types so the rest of this crate can keep using
// `super::models::FocusAnalyzerConfig` etc. unchanged.
//
// #7735 E-2: the `FocusStorage` pass-through re-export (previously
// `pub use maekon_core::ports::focus_storage::FocusStorage;`) was dropped
// from this public path after the move — consumers now import the SSOT
// `maekon_core::ports::focus_storage::FocusStorage` directly. `mod.rs`
// keeps its own internal `use` for the port since `FocusAnalyzer` still
// stores `Arc<dyn FocusStorage>`.
pub use crate::focus_shared::{
    CooldownType, FocusAnalyzerConfig, SessionTracker, SuggestionCooldowns,
};
