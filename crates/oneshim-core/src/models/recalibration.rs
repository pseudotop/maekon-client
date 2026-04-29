//! Recalibration domain models for user-driven regime correction.
//!
//! Supports retroactive override of regime assignments and constraint-based
//! semi-supervised re-clustering.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A user override that corrects a regime assignment for a specific segment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegimeOverride {
    /// Unique identifier for this override.
    pub override_id: String,
    /// The activity segment being overridden.
    pub segment_id: String,
    /// The original regime that was assigned (if any).
    pub original_regime_id: Option<String>,
    /// The corrective action chosen by the user.
    pub user_action: UserOverrideAction,
    /// When this override was created.
    pub created_at: DateTime<Utc>,
}

/// The corrective action a user can apply to a segment's regime assignment.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum UserOverrideAction {
    /// Mark the segment as noise (e.g., personal time, irrelevant activity).
    MarkAsNoise,
    /// Reassign the segment to a different regime.
    ReassignRegime {
        /// The target regime to assign.
        target_regime_id: String,
    },
    /// Mark all segments in a time range as personal time (bulk noise).
    MarkAsPersonalTime {
        /// Start of the personal time range.
        from: DateTime<Utc>,
        /// End of the personal time range.
        to: DateTime<Utc>,
    },
}

/// A constraint applied to the clustering algorithm during re-clustering.
///
/// Constraints translate user overrides into clustering directives.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClusterConstraint {
    /// Force the point at this index to be labeled as noise (-1).
    NoiseLabel(usize),
    /// Force the point at this index into the specified cluster.
    ForceCluster(usize, i32),
    /// Two points must be in the same cluster (Phase 2 — deferred).
    MustLink(usize, usize),
    /// Two points must be in different clusters (Phase 2 — deferred).
    CannotLink(usize, usize),
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn regime_override_serialization_roundtrip() {
        let override_entry = RegimeOverride {
            override_id: "ovr-001".to_string(),
            segment_id: "seg-001".to_string(),
            original_regime_id: Some("regime-0".to_string()),
            user_action: UserOverrideAction::MarkAsNoise,
            created_at: Utc::now(),
        };

        let json = serde_json::to_string(&override_entry).unwrap();
        let deserialized: RegimeOverride = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.override_id, "ovr-001");
        assert_eq!(deserialized.segment_id, "seg-001");
    }

    #[test]
    fn user_override_action_variants_serialize() {
        let noise = UserOverrideAction::MarkAsNoise;
        let json = serde_json::to_string(&noise).unwrap();
        assert!(json.contains("MARK_AS_NOISE"));

        let reassign = UserOverrideAction::ReassignRegime {
            target_regime_id: "regime-2".to_string(),
        };
        let json = serde_json::to_string(&reassign).unwrap();
        assert!(json.contains("REASSIGN_REGIME"));
        assert!(json.contains("regime-2"));

        let personal = UserOverrideAction::MarkAsPersonalTime {
            from: Utc::now(),
            to: Utc::now(),
        };
        let json = serde_json::to_string(&personal).unwrap();
        assert!(json.contains("MARK_AS_PERSONAL_TIME"));
    }

    #[test]
    fn cluster_constraint_variants() {
        let c1 = ClusterConstraint::NoiseLabel(5);
        let c2 = ClusterConstraint::ForceCluster(3, 2);
        let c3 = ClusterConstraint::MustLink(1, 4);
        let c4 = ClusterConstraint::CannotLink(2, 6);

        // Verify Debug formatting works
        assert!(format!("{c1:?}").contains("NoiseLabel"));
        assert!(format!("{c2:?}").contains("ForceCluster"));
        assert!(format!("{c3:?}").contains("MustLink"));
        assert!(format!("{c4:?}").contains("CannotLink"));
    }
}
