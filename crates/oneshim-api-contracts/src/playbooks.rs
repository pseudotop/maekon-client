//! Playbook API contracts — coaching template and automation preset listings.

use serde::{Deserialize, Serialize};

/// DTO for a single coaching template entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoachingTemplateDto {
    pub profile: String,
    pub trigger_type: String,
    pub tone: String,
    pub locale: String,
    pub text: String,
}

/// Response DTO for GET /api/playbooks/coaching.
#[derive(Debug, Serialize)]
pub struct CoachingTemplateListDto {
    pub total: usize,
    pub templates: Vec<CoachingTemplateDto>,
}

/// DTO for a single automation preset summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresetSummaryDto {
    pub id: String,
    pub name: String,
    pub description: String,
    pub category: String,
    pub step_count: usize,
    pub builtin: bool,
}

/// Response DTO for GET /api/playbooks/presets.
#[derive(Debug, Serialize)]
pub struct PresetSummaryListDto {
    pub total: usize,
    pub presets: Vec<PresetSummaryDto>,
}
