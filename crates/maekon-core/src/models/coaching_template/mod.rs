//! Coaching template data — static message templates for coaching profiles.
//!
//! This module provides the `CoachingTemplate` struct and the built-in `TEMPLATES`
//! slice containing 108 templates (54 English + 54 Korean).
//!
//! The data lives in `maekon-core` because it is pure static domain data
//! referenced by both `maekon-analysis` (template selection logic) and
//! `maekon-web` (playbook listing endpoint). Keeping it here avoids a
//! forbidden cross-adapter dependency (web -> analysis).

mod builtin;
mod types;

pub use builtin::TEMPLATES;
pub use types::CoachingTemplate;
