//! Core types for the AI model lifecycle policy subsystem.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModelLifecyclePolicyCatalog {
    pub version: u32,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub rules: Vec<ModelLifecycleRule>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModelLifecycleRule {
    pub provider_type: String,
    #[serde(default)]
    pub surface_id: Option<String>,
    pub model: String,
    #[serde(default)]
    pub warn_at: Option<String>,
    #[serde(default)]
    pub block_at: Option<String>,
    #[serde(default)]
    pub replacement: Option<String>,
    #[serde(default)]
    pub action: ModelLifecycleAction,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ModelLifecycleAction {
    WarnOnly,
    #[default]
    WarnThenBlock,
    Block,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelLifecycleDecision {
    Allowed,
    Warn {
        message: String,
        replacement: Option<String>,
    },
    Block {
        message: String,
        replacement: Option<String>,
    },
}

impl ModelLifecycleDecision {
    pub fn is_blocking(&self) -> bool {
        matches!(self, Self::Block { .. })
    }

    pub fn message(&self) -> Option<&str> {
        match self {
            Self::Allowed => None,
            Self::Warn { message, .. } | Self::Block { message, .. } => Some(message),
        }
    }
}
