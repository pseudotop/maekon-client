//! Suggestion Tauri commands.
//!
//! ADR-013 split: command handlers are kept in focused submodules while this
//! module preserves the public `commands::suggestions::*` invoke paths.

pub(crate) mod action;
pub mod chat_suggestions;
pub(crate) mod feedback;
mod helpers;
mod mapping;
pub(crate) mod queries;
pub(crate) mod replay;
#[cfg(test)]
mod tests;
mod types;
