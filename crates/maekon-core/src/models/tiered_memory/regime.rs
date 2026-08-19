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

/// Resolve the human-readable label for a regime id against a regime list.
///
/// Prefers the `Active` entry for the id (defense-in-depth against a stale
/// duplicate-id `Inactive` entry, mirroring the monitor loop's regime
/// resolution), then falls back to any entry with the id. Returns the `name`
/// when set, otherwise the auto-generated `auto_label`. `None` when the id is
/// absent — callers should pass the absence through to another signal (e.g.
/// `dominant_category`) rather than leaking the opaque positional id
/// ("regime-N") downstream (#7480, #7678 D2).
pub fn resolve_regime_label(regimes: &[Regime], regime_id: &str) -> Option<String> {
    regimes
        .iter()
        .find(|r| r.regime_id == regime_id && r.status == RegimeStatus::Active)
        .or_else(|| regimes.iter().find(|r| r.regime_id == regime_id))
        .map(|r| r.name.clone().unwrap_or_else(|| r.auto_label.clone()))
}

#[cfg(test)]
mod resolve_regime_label_tests {
    use super::*;
    use chrono::Utc;

    fn make_regime(id: &str, name: Option<&str>, auto_label: &str, status: RegimeStatus) -> Regime {
        Regime {
            regime_id: id.to_string(),
            name: name.map(String::from),
            auto_label: auto_label.to_string(),
            centroid: RegimeFeatures::default(),
            optimal_params: TriggerParams::default(),
            sample_count: 1,
            first_seen: Utc::now(),
            last_seen: Utc::now(),
            status,
        }
    }

    #[test]
    fn prefers_name_over_auto_label() {
        let regimes = vec![make_regime(
            "regime-0",
            Some("Deep Focus"),
            "auto-label-0",
            RegimeStatus::Active,
        )];
        assert_eq!(
            resolve_regime_label(&regimes, "regime-0").as_deref(),
            Some("Deep Focus")
        );
    }

    #[test]
    fn falls_back_to_auto_label_when_name_absent() {
        let regimes = vec![make_regime(
            "regime-0",
            None,
            "auto-label-0",
            RegimeStatus::Active,
        )];
        assert_eq!(
            resolve_regime_label(&regimes, "regime-0").as_deref(),
            Some("auto-label-0")
        );
    }

    #[test]
    fn prefers_active_entry_over_stale_duplicate_id_inactive_entry() {
        let regimes = vec![
            make_regime(
                "regime-0",
                Some("Stale Name"),
                "stale",
                RegimeStatus::Inactive,
            ),
            make_regime(
                "regime-0",
                Some("Fresh Name"),
                "fresh",
                RegimeStatus::Active,
            ),
        ];
        assert_eq!(
            resolve_regime_label(&regimes, "regime-0").as_deref(),
            Some("Fresh Name")
        );
    }

    #[test]
    fn unknown_id_returns_none_rather_than_leaking_opaque_id() {
        let regimes = vec![make_regime(
            "regime-0",
            Some("Deep Focus"),
            "auto-label-0",
            RegimeStatus::Active,
        )];
        assert_eq!(resolve_regime_label(&regimes, "regime-9"), None);
    }
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

/// Persisted per-regime user-reaction counts — the restart-surviving form of
/// `RegimeClassifier`'s in-RAM `per_regime_stats` / aggregate `reaction_stats`
/// (#7913 T2.1c). Learned via `record_user_reaction` (#7600) and read back by
/// `acceptance_rate` to progressively quiet regimes the user rejects suggestions
/// in. Before #7913 it was RAM-only and reset on every restart.
///
/// The global aggregate (feedback with no regime context) is persisted under the
/// reserved `regime_id == ""` sentinel: real regime ids are always non-empty
/// (`"regime-N"`), so the empty string cannot collide with a per-regime bucket.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegimeReactionRecord {
    /// Opaque regime id, or `""` for the global aggregate bucket.
    pub regime_id: String,
    pub total: u64,
    pub accepted: u64,
    pub rejected: u64,
    pub deferred: u64,
}

impl RegimeReactionRecord {
    /// The reserved `regime_id` under which the global aggregate is persisted.
    pub const AGGREGATE_KEY: &'static str = "";
}
