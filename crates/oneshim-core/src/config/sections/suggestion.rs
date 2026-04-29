use serde::{Deserialize, Serialize};

/// Configuration for real-time suggestion reception.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SuggestionConfig {
    /// Enable real-time suggestion reception from server.
    #[serde(default)]
    pub enabled: bool,
}
