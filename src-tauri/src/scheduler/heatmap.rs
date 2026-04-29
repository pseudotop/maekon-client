/// Mouse interaction heatmap aggregator — accumulates click positions into
/// a 50×50 grid and applies exponential decay each emit cycle.
/// Used by the overlay HeatmapGhost React component.
const GRID_SIZE: usize = 50;
/// 5% decay per emit cycle — recent activity dominates.
const DECAY_FACTOR: f32 = 0.95;
/// Minimum cell value before zeroing out (noise floor).
const NOISE_FLOOR: f32 = 0.01;

/// Aggregates mouse click positions into a fixed-size grid for overlay rendering.
pub(crate) struct HeatmapAggregator {
    grid: [f32; GRID_SIZE * GRID_SIZE],
    screen_width: u32,
    screen_height: u32,
}

impl HeatmapAggregator {
    pub fn new() -> Self {
        Self {
            grid: [0.0; GRID_SIZE * GRID_SIZE],
            screen_width: 1920,
            screen_height: 1080,
        }
    }

    /// Update screen dimensions for position-to-grid mapping.
    pub fn update_resolution(&mut self, width: u32, height: u32) {
        if width > 0 && height > 0 {
            self.screen_width = width;
            self.screen_height = height;
        }
    }

    /// Record click(s) at the given screen position.
    /// `weight` should be the click count for that position.
    pub fn record(&mut self, x: f32, y: f32, weight: u32) {
        if x < 0.0 || y < 0.0 {
            return;
        }
        let col = ((x / self.screen_width as f32) * GRID_SIZE as f32) as usize;
        let row = ((y / self.screen_height as f32) * GRID_SIZE as f32) as usize;
        let col = col.min(GRID_SIZE - 1);
        let row = row.min(GRID_SIZE - 1);
        self.grid[row * GRID_SIZE + col] += weight as f32;
    }

    /// Returns the grid as a normalized [0.0, 1.0] flat array (row-major, 2500 values)
    /// and applies decay for the next cycle. Returns `None` if the grid is empty.
    pub fn take_snapshot(&mut self) -> Option<Vec<f32>> {
        let max = self.grid.iter().copied().fold(0.0f32, f32::max);
        if max < NOISE_FLOOR {
            return None;
        }

        let normalized: Vec<f32> = self.grid.iter().map(|&v| v / max).collect();

        // Apply exponential decay
        for cell in &mut self.grid {
            *cell *= DECAY_FACTOR;
            if *cell < NOISE_FLOOR {
                *cell = 0.0;
            }
        }

        Some(normalized)
    }

    #[allow(dead_code)] // Public API for tests and future consumers
    pub const fn grid_size() -> usize {
        GRID_SIZE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_aggregator_has_empty_grid() {
        let mut agg = HeatmapAggregator::new();
        assert!(agg.take_snapshot().is_none());
    }

    #[test]
    fn record_and_snapshot() {
        let mut agg = HeatmapAggregator::new();
        agg.update_resolution(1920, 1080);

        // Click at center of screen
        agg.record(960.0, 540.0, 1);
        let snap = agg.take_snapshot().unwrap();
        assert_eq!(snap.len(), GRID_SIZE * GRID_SIZE);

        // Center cell should be 1.0 (max-normalized)
        let center_col = 25;
        let center_row = 25;
        assert!((snap[center_row * GRID_SIZE + center_col] - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn weighted_record() {
        let mut agg = HeatmapAggregator::new();
        agg.update_resolution(1000, 1000);

        // 5 clicks at (100, 100), 1 click at (900, 900)
        agg.record(100.0, 100.0, 5);
        agg.record(900.0, 900.0, 1);

        let snap = agg.take_snapshot().unwrap();
        let cell_a = snap[5 * GRID_SIZE + 5]; // (100/1000)*50 = 5
        let cell_b = snap[45 * GRID_SIZE + 45]; // (900/1000)*50 = 45
        assert!(cell_a > cell_b, "heavier cell should have higher value");
        assert!(
            (cell_a - 1.0).abs() < f32::EPSILON,
            "max cell should be 1.0"
        );
        assert!((cell_b - 0.2).abs() < f32::EPSILON, "1/5 = 0.2");
    }

    #[test]
    fn decay_reduces_values() {
        let mut agg = HeatmapAggregator::new();
        // Default resolution 1920×1080
        // (500/1920)*50 = 13, (500/1080)*50 = 23
        agg.record(500.0, 500.0, 10);

        let _ = agg.take_snapshot(); // applies decay
                                     // After decay: 10 * 0.95 = 9.5
                                     // No new input, next snapshot should still exist (9.5 > noise floor)
        let snap2 = agg.take_snapshot().unwrap();
        assert!((snap2[23 * GRID_SIZE + 13] - 1.0).abs() < f32::EPSILON); // still max

        // After many decays, should eventually return None
        for _ in 0..500 {
            let _ = agg.take_snapshot();
        }
        assert!(agg.take_snapshot().is_none());
    }

    #[test]
    fn negative_positions_ignored() {
        let mut agg = HeatmapAggregator::new();
        agg.record(-10.0, -20.0, 1);
        assert!(agg.take_snapshot().is_none());
    }

    #[test]
    fn resolution_update() {
        let mut agg = HeatmapAggregator::new();
        agg.update_resolution(2560, 1440);

        // Click at (2560, 1440) — edge of screen, should clamp to last cell
        agg.record(2560.0, 1440.0, 1);
        let snap = agg.take_snapshot().unwrap();
        let last_cell = snap[(GRID_SIZE - 1) * GRID_SIZE + (GRID_SIZE - 1)];
        assert!((last_cell - 1.0).abs() < f32::EPSILON);
    }
}
