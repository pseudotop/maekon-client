use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// TriggerParams — CSS-cascade style override bag (all fields optional)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TriggerParams {
    pub w_density: Option<f32>,
    pub w_importance: Option<f32>,
    pub w_context: Option<f32>,
    pub w_buffer: Option<f32>,
    pub alpha_short: Option<f32>,
    pub alpha_long: Option<f32>,
    pub alpha_importance: Option<f32>,
    pub t_high: Option<f32>,
    pub t_low: Option<f32>,
    pub min_segment_secs: Option<u64>,
    pub max_segment_secs: Option<u64>,
    pub buffer_capacity: Option<usize>,
    pub context_decay_rate: Option<f32>,
    pub importance_overrides: Option<HashMap<String, f32>>,
}

// ---------------------------------------------------------------------------
// ResolvedParams — fully resolved (no Options)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolvedParams {
    pub w_density: f32,
    pub w_importance: f32,
    pub w_context: f32,
    pub w_buffer: f32,
    pub alpha_short: f32,
    pub alpha_long: f32,
    pub alpha_importance: f32,
    pub t_high: f32,
    pub t_low: f32,
    pub min_segment_secs: u64,
    pub max_segment_secs: u64,
    pub buffer_capacity: usize,
    pub context_decay_rate: f32,
    pub importance_overrides: HashMap<String, f32>,
}

impl Default for ResolvedParams {
    fn default() -> Self {
        Self {
            w_density: 0.30,
            w_importance: 0.30,
            w_context: 0.25,
            w_buffer: 0.15,
            alpha_short: 0.1,
            alpha_long: 0.01,
            alpha_importance: 0.15,
            t_high: 0.65,
            t_low: 0.25,
            min_segment_secs: 120,
            max_segment_secs: 600,
            buffer_capacity: 100,
            context_decay_rate: 0.85,
            importance_overrides: HashMap::new(),
        }
    }
}

impl ResolvedParams {
    /// Normalize the four score weights so they sum to 1.0.
    /// Clamps individual values to [0.0, 1.0] first; if the total is zero
    /// after clamping, falls back to equal 0.25 each.
    pub fn validate_and_normalize(&mut self) {
        // Clamp weights
        self.w_density = self.w_density.clamp(0.0, 1.0);
        self.w_importance = self.w_importance.clamp(0.0, 1.0);
        self.w_context = self.w_context.clamp(0.0, 1.0);
        self.w_buffer = self.w_buffer.clamp(0.0, 1.0);

        let sum = self.w_density + self.w_importance + self.w_context + self.w_buffer;
        if sum > f32::EPSILON {
            self.w_density /= sum;
            self.w_importance /= sum;
            self.w_context /= sum;
            self.w_buffer /= sum;
        } else {
            self.w_density = 0.25;
            self.w_importance = 0.25;
            self.w_context = 0.25;
            self.w_buffer = 0.25;
        }

        // Clamp EMA alphas
        self.alpha_short = self.alpha_short.clamp(0.0, 1.0);
        self.alpha_long = self.alpha_long.clamp(0.0, 1.0);
        self.alpha_importance = self.alpha_importance.clamp(0.0, 1.0);

        // Clamp thresholds
        self.t_high = self.t_high.clamp(0.0, 1.0);
        self.t_low = self.t_low.clamp(0.0, 1.0);
        if self.t_low > self.t_high {
            std::mem::swap(&mut self.t_low, &mut self.t_high);
        }

        // Ensure min <= max segment duration
        if self.min_segment_secs > self.max_segment_secs {
            std::mem::swap(&mut self.min_segment_secs, &mut self.max_segment_secs);
        }

        // Context decay rate in (0, 1]
        self.context_decay_rate = self.context_decay_rate.clamp(0.01, 1.0);
    }

    /// Merge `Some` fields from `overrides` into self.
    pub fn apply_overrides(&mut self, overrides: &TriggerParams) {
        if let Some(v) = overrides.w_density {
            self.w_density = v;
        }
        if let Some(v) = overrides.w_importance {
            self.w_importance = v;
        }
        if let Some(v) = overrides.w_context {
            self.w_context = v;
        }
        if let Some(v) = overrides.w_buffer {
            self.w_buffer = v;
        }
        if let Some(v) = overrides.alpha_short {
            self.alpha_short = v;
        }
        if let Some(v) = overrides.alpha_long {
            self.alpha_long = v;
        }
        if let Some(v) = overrides.alpha_importance {
            self.alpha_importance = v;
        }
        if let Some(v) = overrides.t_high {
            self.t_high = v;
        }
        if let Some(v) = overrides.t_low {
            self.t_low = v;
        }
        if let Some(v) = overrides.min_segment_secs {
            self.min_segment_secs = v;
        }
        if let Some(v) = overrides.max_segment_secs {
            self.max_segment_secs = v;
        }
        if let Some(v) = overrides.buffer_capacity {
            self.buffer_capacity = v;
        }
        if let Some(v) = overrides.context_decay_rate {
            self.context_decay_rate = v;
        }
        if let Some(ref v) = overrides.importance_overrides {
            for (k, val) in v {
                self.importance_overrides.insert(k.clone(), *val);
            }
        }

        self.validate_and_normalize();
    }
}

// ---------------------------------------------------------------------------
// PresetProfile — role-based parameter defaults
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PresetProfile {
    Developer,
    Manager,
    Designer,
    Researcher,
    #[default]
    General,
}

impl PresetProfile {
    /// Return role-tuned default parameters. The returned params are already
    /// normalized (weights sum to 1.0).
    pub fn default_params(&self) -> ResolvedParams {
        let mut p = match self {
            Self::Developer => ResolvedParams {
                w_density: 0.25,
                w_importance: 0.35,
                w_context: 0.25,
                w_buffer: 0.15,
                t_high: 0.70,
                t_low: 0.20,
                min_segment_secs: 180,
                max_segment_secs: 900,
                importance_overrides: HashMap::from([
                    ("VSCode".to_string(), 0.9),
                    ("Cursor".to_string(), 0.9),
                    ("IntelliJ IDEA".to_string(), 0.9),
                    ("Terminal".to_string(), 0.85),
                    ("iTerm2".to_string(), 0.85),
                    ("Warp".to_string(), 0.85),
                    ("GitHub".to_string(), 0.8),
                ]),
                ..ResolvedParams::default()
            },
            Self::Manager => ResolvedParams {
                w_density: 0.35,
                w_importance: 0.25,
                w_context: 0.25,
                w_buffer: 0.15,
                t_high: 0.55,
                t_low: 0.30,
                min_segment_secs: 90,
                max_segment_secs: 450,
                importance_overrides: HashMap::from([
                    ("Slack".to_string(), 0.9),
                    ("Microsoft Teams".to_string(), 0.9),
                    ("Zoom".to_string(), 0.85),
                    ("Google Meet".to_string(), 0.85),
                    ("Notion".to_string(), 0.8),
                    ("Google Docs".to_string(), 0.8),
                ]),
                ..ResolvedParams::default()
            },
            Self::Designer => ResolvedParams {
                w_density: 0.20,
                w_importance: 0.35,
                w_context: 0.30,
                w_buffer: 0.15,
                t_high: 0.70,
                t_low: 0.20,
                min_segment_secs: 180,
                max_segment_secs: 900,
                importance_overrides: HashMap::from([
                    ("Figma".to_string(), 0.95),
                    ("Sketch".to_string(), 0.9),
                    ("Adobe Photoshop".to_string(), 0.9),
                    ("Adobe Illustrator".to_string(), 0.9),
                    ("Canva".to_string(), 0.8),
                ]),
                ..ResolvedParams::default()
            },
            Self::Researcher => ResolvedParams {
                w_density: 0.20,
                w_importance: 0.30,
                w_context: 0.35,
                w_buffer: 0.15,
                t_high: 0.75,
                t_low: 0.20,
                min_segment_secs: 240,
                max_segment_secs: 1200,
                importance_overrides: HashMap::from([
                    ("Google Chrome".to_string(), 0.85),
                    ("Arc".to_string(), 0.85),
                    ("Firefox".to_string(), 0.85),
                    ("Notion".to_string(), 0.8),
                    ("Obsidian".to_string(), 0.85),
                    ("Zotero".to_string(), 0.9),
                ]),
                ..ResolvedParams::default()
            },
            Self::General => ResolvedParams::default(),
        };
        p.validate_and_normalize();
        p
    }
}
