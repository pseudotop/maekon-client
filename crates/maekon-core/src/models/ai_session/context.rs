//! Context assembly data structures for system prompt generation.

use serde::{Deserialize, Serialize};

// ── Context Assembly ──────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemPromptContext {
    pub user_profile: UserProfileSummary,
    pub current_regime: String,
    pub recent_activity: ActivitySummary,
    pub suggestion_history: SuggestionPatterns,
    pub available_skills: Vec<SkillInfo>,
    pub system_info: SystemInfo,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UserProfileSummary {
    pub preferred_language: Option<String>,
    pub work_style: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ActivitySummary {
    pub top_apps: Vec<String>,
    pub active_minutes: u32,
    pub idle_minutes: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SuggestionPatterns {
    pub total_received: u32,
    pub accepted_count: u32,
    pub rejected_count: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SystemInfo {
    pub os: String,
    pub active_app: Option<String>,
    pub timezone: String,
}
