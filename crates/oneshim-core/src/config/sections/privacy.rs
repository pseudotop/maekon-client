// 개인정보/격리 설정 — PII 필터 수준, 자동화 샌드박스, 제외 앱 목록
use super::super::enums::{PiiFilterLevel, SandboxProfile};
use serde::{Deserialize, Serialize};

// ── PrivacyConfig ──────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrivacyConfig {
    #[serde(default)]
    pub excluded_apps: Vec<String>,
    #[serde(default)]
    pub excluded_app_patterns: Vec<String>,
    #[serde(default)]
    pub excluded_title_patterns: Vec<String>,
    #[serde(default = "default_true")]
    pub auto_exclude_sensitive: bool,
    #[serde(default)]
    pub pii_filter_level: PiiFilterLevel,
}

impl Default for PrivacyConfig {
    fn default() -> Self {
        Self {
            excluded_apps: Vec::new(),
            excluded_app_patterns: Vec::new(),
            excluded_title_patterns: Vec::new(),
            auto_exclude_sensitive: true,
            pii_filter_level: PiiFilterLevel::Standard,
        }
    }
}

// ── SandboxConfig ──────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub profile: SandboxProfile,
    #[serde(default)]
    pub allowed_read_paths: Vec<String>,
    #[serde(default)]
    pub allowed_write_paths: Vec<String>,
    #[serde(default)]
    pub allow_network: bool,
    #[serde(default)]
    pub max_memory_bytes: u64,
    #[serde(default)]
    pub max_cpu_time_ms: u64,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            profile: SandboxProfile::Standard,
            allowed_read_paths: Vec::new(),
            allowed_write_paths: Vec::new(),
            allow_network: false,
            max_memory_bytes: 0,
            max_cpu_time_ms: 0,
        }
    }
}

// ── AutomationConfig ───────────────────────────────────────────────

/// Confirmation policy for automation (RPA) execution.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AutomationConfirmPolicy {
    /// Show confirmation dialog before every execution (safest).
    #[default]
    AlwaysConfirm,
    /// Auto-execute presets marked as trusted; confirm others.
    TrustedOnly,
    /// Execute immediately without confirmation (power users only).
    NeverConfirm,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AutomationConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub sandbox: SandboxConfig,
    #[serde(default)]
    pub custom_presets: Vec<crate::models::intent::WorkflowPreset>,
    #[serde(default)]
    pub confirmation_policy: AutomationConfirmPolicy,
}

// ── Private default helpers ─────────────────────────────────────────

fn default_true() -> bool {
    true
}
