//! Regime lifecycle management (ADR-012 §3).
//!
//! `RegimeManager` is a concrete struct — not a port trait — that owns the
//! set of discovered regimes and applies creation, merge, deactivation, and
//! archival rules.

use chrono::{DateTime, Duration, Utc};
use oneshim_core::config::TieredMemoryConfig;
use oneshim_core::models::tiered_memory::{
    euclidean_distance, Regime, RegimeFeatures, RegimeStatus,
};

use crate::regime_detector::generate_auto_label;

/// Manages the lifecycle of discovered activity regimes.
///
/// Rules (ADR-012 §3):
/// - Creation: new clusters with sufficient samples become Active regimes.
/// - Merge: similar centroids (distance < threshold) AND both below a sample
///   count threshold are merged.
/// - Limit: if more than `max_active` active regimes exist, the closest pair
///   is merged.
/// - Deactivation: regimes not seen for `inactive_days` become Inactive.
/// - Archival: regimes inactive for `archive_days` become Archived.
pub struct RegimeManager {
    regimes: Vec<Regime>,
    max_active: usize,
    inactive_days: u32,
    archive_days: u32,
    merge_distance_threshold: f32,
    min_samples_for_merge: u64,
}

impl RegimeManager {
    /// Create a new manager from config, with sensible defaults.
    pub fn new(_config: &TieredMemoryConfig) -> Self {
        Self {
            regimes: Vec::new(),
            max_active: 7,
            inactive_days: 14,
            archive_days: 30,
            merge_distance_threshold: 0.3,
            min_samples_for_merge: 100,
        }
    }

    /// Create with explicit parameters (useful for testing).
    pub fn with_params(
        max_active: usize,
        inactive_days: u32,
        archive_days: u32,
        merge_distance_threshold: f32,
        min_samples_for_merge: u64,
    ) -> Self {
        Self {
            regimes: Vec::new(),
            max_active,
            inactive_days,
            archive_days,
            merge_distance_threshold,
            min_samples_for_merge,
        }
    }

    /// Replace the in-memory regime list with a persisted snapshot.
    /// Called exactly once at startup from the composition root after
    /// `RegimeStoragePort::load_all`. Does not validate against
    /// `max_active` / `archive_days`; the persisted set is trusted to
    /// be consistent with the config at the time it was saved.
    pub fn hydrate_from(&mut self, regimes: Vec<Regime>) {
        self.regimes = regimes;
    }

    /// Update regimes from `RegimeDetector` output. Applies lifecycle rules:
    /// 1. Merge detected regimes into existing set (match by closest centroid)
    /// 2. Apply merge rule (similar centroids AND both below min_samples_for_merge)
    /// 3. Apply limit rule (> max_active → merge closest pair)
    pub fn update_from_detection(&mut self, detected: Vec<Regime>) {
        for det in detected {
            // Try to match with existing active regime
            let best_match = self
                .regimes
                .iter()
                .enumerate()
                .filter(|(_, r)| r.status == RegimeStatus::Active)
                .min_by(|(_, a), (_, b)| {
                    euclidean_distance(&a.centroid, &det.centroid)
                        .partial_cmp(&euclidean_distance(&b.centroid, &det.centroid))
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|(i, r)| (i, euclidean_distance(&r.centroid, &det.centroid)));

            if let Some((idx, dist)) = best_match {
                if dist < self.merge_distance_threshold {
                    // Update existing regime's centroid (weighted average)
                    let existing = &mut self.regimes[idx];
                    existing.centroid = weighted_centroid(
                        &existing.centroid,
                        existing.sample_count,
                        &det.centroid,
                        det.sample_count,
                    );
                    existing.sample_count += det.sample_count;
                    existing.last_seen = det.last_seen;
                    existing.auto_label = generate_auto_label(&existing.centroid, &[]);
                    continue;
                }
            }

            // No close match — add as new regime
            self.regimes.push(det);
        }

        // Merge similar small regimes
        self.merge_similar_small_regimes();

        // Enforce max_active limit
        self.enforce_max_active();
    }

    /// Merge two regimes that are similar (distance < threshold) and both
    /// have fewer than `min_samples_for_merge` samples.
    fn merge_similar_small_regimes(&mut self) {
        loop {
            let pair = self.find_mergeable_pair();
            if let Some((i, j)) = pair {
                let merged = self.merge_two(i, j);
                // Remove higher index first to avoid invalidating the lower
                let (lo, hi) = if i < j { (i, j) } else { (j, i) };
                self.regimes.remove(hi);
                self.regimes.remove(lo);
                self.regimes.push(merged);
            } else {
                break;
            }
        }
    }

    /// Find the first pair of active regimes that are close AND both below
    /// the sample threshold.
    fn find_mergeable_pair(&self) -> Option<(usize, usize)> {
        let active: Vec<usize> = self
            .regimes
            .iter()
            .enumerate()
            .filter(|(_, r)| r.status == RegimeStatus::Active)
            .map(|(i, _)| i)
            .collect();

        for (ai, &i) in active.iter().enumerate() {
            for &j in &active[ai + 1..] {
                let ri = &self.regimes[i];
                let rj = &self.regimes[j];
                if ri.sample_count < self.min_samples_for_merge
                    && rj.sample_count < self.min_samples_for_merge
                    && euclidean_distance(&ri.centroid, &rj.centroid)
                        < self.merge_distance_threshold
                {
                    return Some((i, j));
                }
            }
        }
        None
    }

    /// Enforce the max_active limit by merging the closest pair until within limit.
    fn enforce_max_active(&mut self) {
        while self.active_count() > self.max_active {
            if let Some((i, j)) = self.find_closest_active_pair() {
                let merged = self.merge_two(i, j);
                let (lo, hi) = if i < j { (i, j) } else { (j, i) };
                self.regimes.remove(hi);
                self.regimes.remove(lo);
                self.regimes.push(merged);
            } else {
                break;
            }
        }
    }

    /// Find the closest pair of active regimes by centroid distance.
    fn find_closest_active_pair(&self) -> Option<(usize, usize)> {
        let active: Vec<usize> = self
            .regimes
            .iter()
            .enumerate()
            .filter(|(_, r)| r.status == RegimeStatus::Active)
            .map(|(i, _)| i)
            .collect();

        let mut best: Option<(usize, usize, f32)> = None;
        for (ai, &i) in active.iter().enumerate() {
            for &j in &active[ai + 1..] {
                let d = euclidean_distance(&self.regimes[i].centroid, &self.regimes[j].centroid);
                if best.as_ref().map_or(true, |b| d < b.2) {
                    best = Some((i, j, d));
                }
            }
        }
        best.map(|(i, j, _)| (i, j))
    }

    /// Merge two regimes into one. The centroid is the weighted average.
    /// The optimal_params come from the regime with more samples.
    /// The name comes from the user-named one (if any) or the larger regime's label.
    fn merge_two(&self, i: usize, j: usize) -> Regime {
        let a = &self.regimes[i];
        let b = &self.regimes[j];

        let (larger, _smaller) = if a.sample_count >= b.sample_count {
            (a, b)
        } else {
            (b, a)
        };

        let centroid = weighted_centroid(&a.centroid, a.sample_count, &b.centroid, b.sample_count);

        // Name: prefer user-given name
        let name = a.name.clone().or_else(|| b.name.clone());

        let auto_label = generate_auto_label(&centroid, &[]);

        Regime {
            regime_id: larger.regime_id.clone(),
            name,
            auto_label,
            centroid,
            optimal_params: larger.optimal_params.clone(),
            sample_count: a.sample_count + b.sample_count,
            first_seen: a.first_seen.min(b.first_seen),
            last_seen: a.last_seen.max(b.last_seen),
            status: RegimeStatus::Active,
        }
    }

    fn active_count(&self) -> usize {
        self.regimes
            .iter()
            .filter(|r| r.status == RegimeStatus::Active)
            .count()
    }

    /// Mark a regime as seen (update last_seen timestamp).
    pub fn mark_seen(&mut self, regime_id: &str, timestamp: DateTime<Utc>) {
        if let Some(r) = self.regimes.iter_mut().find(|r| r.regime_id == regime_id) {
            r.last_seen = timestamp;
        }
    }

    /// Run periodic maintenance (deactivation + archival).
    pub fn run_maintenance(&mut self, now: DateTime<Utc>) {
        let inactive_cutoff = now - Duration::days(i64::from(self.inactive_days));
        let archive_cutoff = now - Duration::days(i64::from(self.archive_days));

        for regime in &mut self.regimes {
            match regime.status {
                RegimeStatus::Active => {
                    if regime.last_seen < inactive_cutoff {
                        regime.status = RegimeStatus::Inactive;
                    }
                }
                RegimeStatus::Inactive => {
                    if regime.last_seen < archive_cutoff {
                        regime.status = RegimeStatus::Archived;
                    }
                }
                RegimeStatus::Archived => {}
            }
        }
    }

    /// Get all active regimes.
    pub fn active_regimes(&self) -> Vec<&Regime> {
        self.regimes
            .iter()
            .filter(|r| r.status == RegimeStatus::Active)
            .collect()
    }

    /// Get all regimes (any status).
    pub fn all_regimes(&self) -> &[Regime] {
        &self.regimes
    }

    /// User override: rename a regime.
    pub fn rename(&mut self, regime_id: &str, name: String) {
        if let Some(r) = self.regimes.iter_mut().find(|r| r.regime_id == regime_id) {
            r.name = Some(name);
        }
    }

    /// User override: delete a regime.
    pub fn delete(&mut self, regime_id: &str) {
        self.regimes.retain(|r| r.regime_id != regime_id);
    }

    /// User override: merge two regimes by ID.
    pub fn merge(&mut self, regime_id_a: &str, regime_id_b: &str) {
        let idx_a = self.regimes.iter().position(|r| r.regime_id == regime_id_a);
        let idx_b = self.regimes.iter().position(|r| r.regime_id == regime_id_b);

        if let (Some(i), Some(j)) = (idx_a, idx_b) {
            if i != j {
                let merged = self.merge_two(i, j);
                let (lo, hi) = if i < j { (i, j) } else { (j, i) };
                self.regimes.remove(hi);
                self.regimes.remove(lo);
                self.regimes.push(merged);
            }
        }
    }
}

/// Compute a weighted-average centroid from two regime centroids.
fn weighted_centroid(
    a: &RegimeFeatures,
    count_a: u64,
    b: &RegimeFeatures,
    count_b: u64,
) -> RegimeFeatures {
    let total = (count_a + count_b) as f32;
    if total == 0.0 {
        return a.clone();
    }
    let wa = count_a as f32 / total;
    let wb = count_b as f32 / total;

    let aa = a.to_array();
    let bb = b.to_array();
    let mut out = [0.0f32; RegimeFeatures::DIMENSIONS];
    for d in 0..RegimeFeatures::DIMENSIONS {
        out[d] = aa[d] * wa + bb[d] * wb;
    }
    RegimeFeatures::from_array(out)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use oneshim_core::config::TieredMemoryConfig;
    use oneshim_core::models::tiered_memory::TriggerParams;

    fn make_regime(
        id: &str,
        centroid: RegimeFeatures,
        sample_count: u64,
        status: RegimeStatus,
    ) -> Regime {
        Regime {
            regime_id: id.to_string(),
            name: None,
            auto_label: generate_auto_label(&centroid, &[]),
            centroid,
            optimal_params: TriggerParams::default(),
            sample_count,
            first_seen: Utc::now(),
            last_seen: Utc::now(),
            status,
        }
    }

    fn coding_centroid() -> RegimeFeatures {
        RegimeFeatures {
            category_coding: 1.0,
            avg_event_rate: 0.3,
            avg_importance: 0.8,
            context_activity_signal: 0.1,
            communication_ratio: 0.05,
            ..RegimeFeatures::default()
        }
    }

    fn comm_centroid() -> RegimeFeatures {
        RegimeFeatures {
            category_communication: 1.0,
            avg_event_rate: 0.7,
            avg_importance: 0.4,
            context_activity_signal: 0.5,
            communication_ratio: 0.8,
            ..RegimeFeatures::default()
        }
    }

    fn browser_centroid() -> RegimeFeatures {
        RegimeFeatures {
            category_browser: 1.0,
            avg_event_rate: 0.5,
            avg_importance: 0.5,
            context_activity_signal: 0.3,
            communication_ratio: 0.15,
            ..RegimeFeatures::default()
        }
    }

    #[test]
    fn creation_from_detection() {
        let config = TieredMemoryConfig::default();
        let mut mgr = RegimeManager::new(&config);

        let detected = vec![
            make_regime("r-0", coding_centroid(), 200, RegimeStatus::Active),
            make_regime("r-1", comm_centroid(), 150, RegimeStatus::Active),
        ];

        mgr.update_from_detection(detected);

        assert_eq!(mgr.active_regimes().len(), 2);
        assert_eq!(mgr.all_regimes().len(), 2);
    }

    #[test]
    fn merge_similar_regimes() {
        // Two regimes with very similar centroids and both below min_samples
        let mut mgr = RegimeManager::with_params(7, 14, 30, 0.5, 200);

        let slightly_different = RegimeFeatures {
            category_coding: 0.95,
            avg_event_rate: 0.32,
            avg_importance: 0.78,
            context_activity_signal: 0.12,
            communication_ratio: 0.06,
            ..RegimeFeatures::default()
        };

        let detected = vec![
            make_regime("r-0", coding_centroid(), 50, RegimeStatus::Active),
            make_regime("r-1", slightly_different, 30, RegimeStatus::Active),
        ];

        mgr.update_from_detection(detected);

        // Should have merged into one regime
        assert_eq!(mgr.active_regimes().len(), 1);
        let merged = &mgr.active_regimes()[0];
        assert_eq!(merged.sample_count, 80);
    }

    #[test]
    fn deactivation_after_n_days() {
        let config = TieredMemoryConfig::default();
        let mut mgr = RegimeManager::new(&config);

        let old_time = Utc::now() - Duration::days(20);
        let mut regime = make_regime("r-0", coding_centroid(), 200, RegimeStatus::Active);
        regime.last_seen = old_time;
        mgr.regimes.push(regime);

        mgr.run_maintenance(Utc::now());

        assert_eq!(mgr.all_regimes()[0].status, RegimeStatus::Inactive);
    }

    #[test]
    fn archival_after_n_days_inactive() {
        let config = TieredMemoryConfig::default();
        let mut mgr = RegimeManager::new(&config);

        let old_time = Utc::now() - Duration::days(35);
        let mut regime = make_regime("r-0", coding_centroid(), 200, RegimeStatus::Inactive);
        regime.last_seen = old_time;
        mgr.regimes.push(regime);

        mgr.run_maintenance(Utc::now());

        assert_eq!(mgr.all_regimes()[0].status, RegimeStatus::Archived);
    }

    #[test]
    fn max_active_limit_enforcement() {
        let mut mgr = RegimeManager::with_params(2, 14, 30, 0.3, 1000);

        let detected = vec![
            make_regime("r-0", coding_centroid(), 200, RegimeStatus::Active),
            make_regime("r-1", comm_centroid(), 150, RegimeStatus::Active),
            make_regime("r-2", browser_centroid(), 100, RegimeStatus::Active),
        ];

        mgr.update_from_detection(detected);

        // max_active is 2, so one pair should have been merged
        assert!(mgr.active_regimes().len() <= 2);
    }

    #[test]
    fn user_rename() {
        let config = TieredMemoryConfig::default();
        let mut mgr = RegimeManager::new(&config);
        mgr.regimes.push(make_regime(
            "r-0",
            coding_centroid(),
            200,
            RegimeStatus::Active,
        ));

        mgr.rename("r-0", "My Coding Mode".to_string());

        assert_eq!(
            mgr.all_regimes()[0].name,
            Some("My Coding Mode".to_string())
        );
    }

    #[test]
    fn user_delete() {
        let config = TieredMemoryConfig::default();
        let mut mgr = RegimeManager::new(&config);
        mgr.regimes.push(make_regime(
            "r-0",
            coding_centroid(),
            200,
            RegimeStatus::Active,
        ));
        mgr.regimes.push(make_regime(
            "r-1",
            comm_centroid(),
            150,
            RegimeStatus::Active,
        ));

        mgr.delete("r-0");

        assert_eq!(mgr.all_regimes().len(), 1);
        assert_eq!(mgr.all_regimes()[0].regime_id, "r-1");
    }

    #[test]
    fn user_merge() {
        let config = TieredMemoryConfig::default();
        let mut mgr = RegimeManager::new(&config);
        mgr.regimes.push(make_regime(
            "r-0",
            coding_centroid(),
            200,
            RegimeStatus::Active,
        ));
        mgr.regimes.push(make_regime(
            "r-1",
            comm_centroid(),
            150,
            RegimeStatus::Active,
        ));

        mgr.merge("r-0", "r-1");

        assert_eq!(mgr.all_regimes().len(), 1);
        assert_eq!(mgr.all_regimes()[0].sample_count, 350);
    }

    #[test]
    fn mark_seen_updates_timestamp() {
        let config = TieredMemoryConfig::default();
        let mut mgr = RegimeManager::new(&config);
        let old_time = Utc::now() - Duration::hours(5);
        let mut regime = make_regime("r-0", coding_centroid(), 200, RegimeStatus::Active);
        regime.last_seen = old_time;
        mgr.regimes.push(regime);

        let now = Utc::now();
        mgr.mark_seen("r-0", now);

        assert!(mgr.all_regimes()[0].last_seen >= now - Duration::seconds(1));
    }

    #[test]
    fn weighted_centroid_equal_weights() {
        let a = coding_centroid();
        let b = comm_centroid();

        let c = weighted_centroid(&a, 100, &b, 100);
        // Each dimension should be the mean
        let expected_coding = (a.category_coding + b.category_coding) / 2.0;
        assert!((c.category_coding - expected_coding).abs() < 1e-5);
    }

    #[test]
    fn weighted_centroid_unequal_weights() {
        let a = coding_centroid();
        let b = comm_centroid();

        let c = weighted_centroid(&a, 300, &b, 100);
        // Should be closer to a
        let dist_a = euclidean_distance(&c, &a);
        let dist_b = euclidean_distance(&c, &b);
        assert!(dist_a < dist_b);
    }

    #[test]
    fn merge_preserves_user_name() {
        let config = TieredMemoryConfig::default();
        let mut mgr = RegimeManager::new(&config);

        let mut r0 = make_regime("r-0", coding_centroid(), 100, RegimeStatus::Active);
        r0.name = Some("My Focus".to_string());
        mgr.regimes.push(r0);
        mgr.regimes.push(make_regime(
            "r-1",
            coding_centroid(),
            200,
            RegimeStatus::Active,
        ));

        mgr.merge("r-0", "r-1");

        let merged = &mgr.all_regimes()[0];
        assert_eq!(merged.name, Some("My Focus".to_string()));
    }

    #[test]
    fn active_regime_not_deactivated_if_recently_seen() {
        let config = TieredMemoryConfig::default();
        let mut mgr = RegimeManager::new(&config);

        let recent = make_regime("r-0", coding_centroid(), 200, RegimeStatus::Active);
        mgr.regimes.push(recent);

        mgr.run_maintenance(Utc::now());

        assert_eq!(mgr.all_regimes()[0].status, RegimeStatus::Active);
    }

    /// T-C3c-5 — `hydrate_from` replaces the in-memory regime list with a
    /// persisted snapshot (composition-root startup path).
    #[test]
    fn hydrate_from_replaces_in_memory_state() {
        let cfg = TieredMemoryConfig::default();
        let mut mgr = RegimeManager::new(&cfg);
        assert_eq!(mgr.all_regimes().len(), 0, "sanity: empty to start");

        let imported = vec![
            make_regime("r1", coding_centroid(), 10, RegimeStatus::Active),
            make_regime("r2", comm_centroid(), 5, RegimeStatus::Active),
        ];
        mgr.hydrate_from(imported);
        assert_eq!(mgr.all_regimes().len(), 2);
        assert_eq!(mgr.all_regimes()[0].regime_id, "r1");
        assert_eq!(mgr.all_regimes()[1].regime_id, "r2");
    }
}
