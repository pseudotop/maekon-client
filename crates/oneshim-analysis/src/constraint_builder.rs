//! Translates user regime overrides into clustering constraints.
//!
//! Used by the recalibration engine to convert `RegimeOverride` entries
//! into `ClusterConstraint` directives for `ClusteringStrategy::detect_with_constraints`.

use std::collections::HashMap;

use oneshim_core::models::recalibration::{ClusterConstraint, RegimeOverride, UserOverrideAction};
use tracing::debug;

/// Build clustering constraints from user overrides.
///
/// # Arguments
/// - `overrides`: The user-created regime overrides.
/// - `feature_indices`: Mapping from segment_id to the index in the feature vector array.
/// - `regime_cluster_map`: Mapping from regime_id to cluster_id (for ReassignRegime).
///
/// Segments not present in `feature_indices` are silently skipped (they may
/// belong to a different clustering window).
pub fn build_constraints(
    overrides: &[RegimeOverride],
    feature_indices: &HashMap<String, usize>,
    regime_cluster_map: &HashMap<String, i32>,
) -> Vec<ClusterConstraint> {
    let mut constraints = Vec::new();

    for entry in overrides {
        match &entry.user_action {
            UserOverrideAction::MarkAsNoise => {
                if let Some(&idx) = feature_indices.get(&entry.segment_id) {
                    constraints.push(ClusterConstraint::NoiseLabel(idx));
                    debug!(
                        segment_id = %entry.segment_id,
                        idx,
                        "MarkAsNoise → NoiseLabel constraint"
                    );
                }
            }
            UserOverrideAction::ReassignRegime { target_regime_id } => {
                if let Some(&idx) = feature_indices.get(&entry.segment_id) {
                    if let Some(&cluster_id) = regime_cluster_map.get(target_regime_id) {
                        constraints.push(ClusterConstraint::ForceCluster(idx, cluster_id));
                        debug!(
                            segment_id = %entry.segment_id,
                            idx,
                            cluster_id,
                            "ReassignRegime → ForceCluster constraint"
                        );
                    } else {
                        debug!(
                            segment_id = %entry.segment_id,
                            target_regime_id,
                            "ReassignRegime skipped — target regime not in cluster map"
                        );
                    }
                }
            }
            UserOverrideAction::MarkAsPersonalTime { .. } => {
                // For MarkAsPersonalTime, the segment_id in the override
                // refers to the initiating segment. All segments in the
                // time range should already be resolved to individual
                // overrides by the caller. If this segment is in our
                // feature set, mark it as noise.
                if let Some(&idx) = feature_indices.get(&entry.segment_id) {
                    constraints.push(ClusterConstraint::NoiseLabel(idx));
                    debug!(
                        segment_id = %entry.segment_id,
                        idx,
                        "MarkAsPersonalTime → NoiseLabel constraint"
                    );
                }
            }
        }
    }

    constraints
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn make_override(id: &str, segment_id: &str, action: UserOverrideAction) -> RegimeOverride {
        RegimeOverride {
            override_id: id.to_string(),
            segment_id: segment_id.to_string(),
            original_regime_id: None,
            user_action: action,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn mark_as_noise_produces_noise_label() {
        let overrides = vec![make_override(
            "o1",
            "seg-a",
            UserOverrideAction::MarkAsNoise,
        )];

        let mut feature_indices = HashMap::new();
        feature_indices.insert("seg-a".to_string(), 3);

        let constraints = build_constraints(&overrides, &feature_indices, &HashMap::new());

        assert_eq!(constraints.len(), 1);
        assert!(matches!(constraints[0], ClusterConstraint::NoiseLabel(3)));
    }

    #[test]
    fn reassign_regime_produces_force_cluster() {
        let overrides = vec![make_override(
            "o2",
            "seg-b",
            UserOverrideAction::ReassignRegime {
                target_regime_id: "regime-2".to_string(),
            },
        )];

        let mut feature_indices = HashMap::new();
        feature_indices.insert("seg-b".to_string(), 7);

        let mut regime_cluster_map = HashMap::new();
        regime_cluster_map.insert("regime-2".to_string(), 1);

        let constraints = build_constraints(&overrides, &feature_indices, &regime_cluster_map);

        assert_eq!(constraints.len(), 1);
        assert!(matches!(
            constraints[0],
            ClusterConstraint::ForceCluster(7, 1)
        ));
    }

    #[test]
    fn reassign_regime_missing_cluster_map_skipped() {
        let overrides = vec![make_override(
            "o3",
            "seg-c",
            UserOverrideAction::ReassignRegime {
                target_regime_id: "regime-unknown".to_string(),
            },
        )];

        let mut feature_indices = HashMap::new();
        feature_indices.insert("seg-c".to_string(), 0);

        let constraints = build_constraints(&overrides, &feature_indices, &HashMap::new());

        assert!(constraints.is_empty());
    }

    #[test]
    fn mark_as_personal_time_produces_noise_label() {
        let now = Utc::now();
        let overrides = vec![make_override(
            "o4",
            "seg-d",
            UserOverrideAction::MarkAsPersonalTime { from: now, to: now },
        )];

        let mut feature_indices = HashMap::new();
        feature_indices.insert("seg-d".to_string(), 5);

        let constraints = build_constraints(&overrides, &feature_indices, &HashMap::new());

        assert_eq!(constraints.len(), 1);
        assert!(matches!(constraints[0], ClusterConstraint::NoiseLabel(5)));
    }

    #[test]
    fn unknown_segment_silently_skipped() {
        let overrides = vec![make_override(
            "o5",
            "seg-unknown",
            UserOverrideAction::MarkAsNoise,
        )];

        let feature_indices = HashMap::new(); // empty — no matching segments

        let constraints = build_constraints(&overrides, &feature_indices, &HashMap::new());

        assert!(constraints.is_empty());
    }

    #[test]
    fn multiple_overrides_combined() {
        let now = Utc::now();
        let overrides = vec![
            make_override("o1", "seg-a", UserOverrideAction::MarkAsNoise),
            make_override(
                "o2",
                "seg-b",
                UserOverrideAction::ReassignRegime {
                    target_regime_id: "regime-1".to_string(),
                },
            ),
            make_override(
                "o3",
                "seg-c",
                UserOverrideAction::MarkAsPersonalTime { from: now, to: now },
            ),
        ];

        let mut feature_indices = HashMap::new();
        feature_indices.insert("seg-a".to_string(), 0);
        feature_indices.insert("seg-b".to_string(), 1);
        feature_indices.insert("seg-c".to_string(), 2);

        let mut regime_cluster_map = HashMap::new();
        regime_cluster_map.insert("regime-1".to_string(), 0);

        let constraints = build_constraints(&overrides, &feature_indices, &regime_cluster_map);

        assert_eq!(constraints.len(), 3);
        assert!(matches!(constraints[0], ClusterConstraint::NoiseLabel(0)));
        assert!(matches!(
            constraints[1],
            ClusterConstraint::ForceCluster(1, 0)
        ));
        assert!(matches!(constraints[2], ClusterConstraint::NoiseLabel(2)));
    }
}
