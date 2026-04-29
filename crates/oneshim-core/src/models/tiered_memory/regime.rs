use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::params::TriggerParams;

// ---------------------------------------------------------------------------
// RegimeFeatures — feature vector for regime clustering
// ---------------------------------------------------------------------------

/// Feature vector for regime clustering.
/// Categorical features are one-hot encoded for Euclidean distance compatibility.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct RegimeFeatures {
    /// One-hot: dominant category is coding
    pub category_coding: f32,
    /// One-hot: dominant category is communication
    pub category_communication: f32,
    /// One-hot: dominant category is browser
    pub category_browser: f32,
    /// Normalized [0-1] average event rate
    pub avg_event_rate: f32,
    /// Normalized [0-1] average event importance
    pub avg_importance: f32,
    /// Decaying context signal from AdaptiveTrigger (app switches, idle transitions).
    pub context_activity_signal: f32,
    /// Ratio of communication-category events
    pub communication_ratio: f32,
}

impl RegimeFeatures {
    /// Number of dimensions in the feature vector.
    pub const DIMENSIONS: usize = 7;

    /// Convert to a fixed-size array for distance computation.
    pub fn to_array(&self) -> [f32; Self::DIMENSIONS] {
        [
            self.category_coding,
            self.category_communication,
            self.category_browser,
            self.avg_event_rate,
            self.avg_importance,
            self.context_activity_signal,
            self.communication_ratio,
        ]
    }

    /// Construct from a fixed-size array.
    pub fn from_array(arr: [f32; Self::DIMENSIONS]) -> Self {
        Self {
            category_coding: arr[0],
            category_communication: arr[1],
            category_browser: arr[2],
            avg_event_rate: arr[3],
            avg_importance: arr[4],
            context_activity_signal: arr[5],
            communication_ratio: arr[6],
        }
    }
}

/// Compute Euclidean distance between two feature vectors.
pub fn euclidean_distance(a: &RegimeFeatures, b: &RegimeFeatures) -> f32 {
    let aa = a.to_array();
    let bb = b.to_array();
    aa.iter()
        .zip(bb.iter())
        .map(|(x, y)| (x - y) * (x - y))
        .sum::<f32>()
        .sqrt()
}

// ---------------------------------------------------------------------------
// RegimeStatus / Regime
// ---------------------------------------------------------------------------

/// Lifecycle status of a discovered regime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RegimeStatus {
    Active,
    Inactive,
    Archived,
}

/// A discovered activity regime (work mode cluster).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Regime {
    pub regime_id: String,
    /// User-provided name (overrides auto_label in UI).
    pub name: Option<String>,
    /// Auto-generated human-readable label from centroid features.
    pub auto_label: String,
    /// Cluster centroid in feature space.
    pub centroid: RegimeFeatures,
    /// Optimal trigger params derived from cluster statistics (Option fields for cascade).
    pub optimal_params: TriggerParams,
    /// Number of data points in this cluster.
    pub sample_count: u64,
    /// When this regime was first observed.
    pub first_seen: DateTime<Utc>,
    /// When this regime was last classified.
    pub last_seen: DateTime<Utc>,
    /// Lifecycle status.
    pub status: RegimeStatus,
}

// ---------------------------------------------------------------------------
// RegimeNotification — regime transition events
// ---------------------------------------------------------------------------

/// Notification produced when the active regime changes or a new one is discovered.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RegimeNotification {
    /// The active regime changed from one to another (or from None).
    RegimeChanged { from: Option<String>, to: String },
    /// A new regime was discovered during detection.
    RegimeDiscovered { label: String },
}
