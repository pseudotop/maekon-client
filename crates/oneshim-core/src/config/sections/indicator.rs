use serde::{Deserialize, Serialize};

/// Tracking overlay indicator preferences — border highlight and floating panel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndicatorConfig {
    pub show_border: bool,
    pub show_panel: bool,
    pub border_opacity: f32,
}

impl Default for IndicatorConfig {
    fn default() -> Self {
        Self {
            show_border: true,
            show_panel: true,
            border_opacity: 0.6,
        }
    }
}
