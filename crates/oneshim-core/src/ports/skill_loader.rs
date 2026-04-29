//! Port for discovering and loading skill definitions.

use crate::error::CoreError;
use crate::models::skill::{Skill, SkillMeta};

/// Discovers SKILL.md files and parses their frontmatter/body.
///
/// # Errors
/// - `CoreError::NotFound` (wire: `resource.not_found`, `resource_type =
///   "Skill"`) — `get_skill` with an unknown skill name. This is the
///   only Err variant the current reference adapter (`FileSkillLoader`
///   in `src-tauri`) emits.
/// - `list_skills` is infallible by return type (no `Result<_, _>`).
/// - `reload` is declared fallible but the current adapter tolerates
///   directory-read and per-file parse failures (logged + skipped),
///   always returning `Ok(())`. Future adapters MAY surface
///   `CoreError::Io` (wire: `internal.io`) for durable I/O failures
///   or `CoreError::Internal` (wire: `internal.generic`) for
///   frontmatter parse / lock poisoning — callers should not assume
///   `reload` is infallible even though today's impl is.
pub trait SkillLoader: Send + Sync {
    /// List all available skill metadata (name + description only).
    /// Used for progressive disclosure in system prompts.
    fn list_skills(&self) -> Vec<SkillMeta>;

    /// Load a skill by name, including its full body content.
    fn get_skill(&self, name: &str) -> Result<Skill, CoreError>;

    /// Reload skills from disk (e.g., after config change).
    fn reload(&self) -> Result<(), CoreError>;
}
