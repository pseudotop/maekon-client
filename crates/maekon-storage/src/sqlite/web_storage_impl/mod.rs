//! `WebStorage` port implementations for `SqliteStorage`.
//!
//! ADR-013 split: 1,045-line `web_storage_impl.rs` → 5 focused submodules.
//!
//! | Submodule | Traits implemented |
//! |-----------|--------------------|
//! | `event_frame_storage` | `EventQueryStorage`, `FrameQueryStorage`, `StorageMaintenanceStorage` |
//! | `tag_activity_focus_storage` | `TagStorage`, `ActivityStatsStorage`, `FocusQueryStorage` |
//! | `suggestion_digest_storage` | `SuggestionQueryStorage`, `DigestStorage` |
//! | `backup_segment_gui_storage` | `BackupStorage`, `SegmentQueryStorage`, `GuiInteractionStorage` |
//! | `coaching_habit_storage` | `CoachingQueryStorage`, `HabitStorage` |

mod backup_segment_gui_storage;
mod coaching_habit_storage;
mod event_frame_storage;
pub(crate) mod suggestion_digest_storage;
mod tag_activity_focus_storage;

use chrono::Utc;
use maekon_core::error::CoreError;
use maekon_core::models::daily_digest::DailyDigest;

use super::SqliteStorage;

impl SqliteStorage {
    /// Parse a daily digest row from its constituent JSON columns.
    pub(super) fn parse_daily_digest_row(
        date_str: &str,
        insight_json: Option<&str>,
        timeline_json: &str,
        statistics_json: &str,
        generated_at_str: &str,
    ) -> Result<DailyDigest, CoreError> {
        let date = chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d").map_err(|e| {
            CoreError::Storage {
                code: maekon_core::error_codes::StorageCode::Failed,
                message: format!("Invalid date in daily_digests: {e}"),
            }
        })?;
        let insight = insight_json
            .map(serde_json::from_str)
            .transpose()
            .map_err(|e| CoreError::Storage {
                code: maekon_core::error_codes::StorageCode::Failed,
                message: format!("Failed to deserialize insight: {e}"),
            })?;
        let timeline = serde_json::from_str(timeline_json).map_err(|e| CoreError::Storage {
            code: maekon_core::error_codes::StorageCode::Failed,
            message: format!("Failed to deserialize timeline: {e}"),
        })?;
        let statistics = serde_json::from_str(statistics_json).map_err(|e| CoreError::Storage {
            code: maekon_core::error_codes::StorageCode::Failed,
            message: format!("Failed to deserialize statistics: {e}"),
        })?;
        let generated_at = chrono::DateTime::parse_from_rfc3339(generated_at_str)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());

        Ok(DailyDigest {
            date,
            insight,
            timeline,
            statistics,
            generated_at,
        })
    }
}

/// Parse the centroid (center x, center y) from a bbox JSON string.
///
/// Accepts:
/// - `{"x":100,"y":200,"w":50,"h":30}` → centroid (125, 215)
/// - `{"x":100,"y":200}` → (100, 200)
/// - `[100, 200, 150, 230]` (x, y, x2, y2) → centroid (125, 215)
#[cfg(test)]
pub(super) fn parse_bbox_centroid(json: &str) -> Option<(u32, u32)> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    if let Some(obj) = v.as_object() {
        let x = obj.get("x").and_then(|v| v.as_f64())? as u32;
        let y = obj.get("y").and_then(|v| v.as_f64())? as u32;
        let w = obj.get("w").and_then(|v| v.as_f64()).unwrap_or(0.0) as u32;
        let h = obj.get("h").and_then(|v| v.as_f64()).unwrap_or(0.0) as u32;
        Some((x + w / 2, y + h / 2))
    } else if let Some(arr) = v.as_array() {
        if arr.len() >= 4 {
            let x1 = arr[0].as_f64()? as u32;
            let y1 = arr[1].as_f64()? as u32;
            let x2 = arr[2].as_f64()? as u32;
            let y2 = arr[3].as_f64()? as u32;
            Some(((x1 + x2) / 2, (y1 + y2) / 2))
        } else if arr.len() >= 2 {
            let x = arr[0].as_f64()? as u32;
            let y = arr[1].as_f64()? as u32;
            Some((x, y))
        } else {
            None
        }
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::parse_bbox_centroid;

    #[test]
    fn object_with_xywh_returns_centroid() {
        // x=100, y=200, w=50, h=30 → centroid = (125, 215)
        let json = r#"{"x":100,"y":200,"w":50,"h":30}"#;
        assert_eq!(parse_bbox_centroid(json), Some((125, 215)));
    }

    #[test]
    fn object_with_xy_only_returns_point() {
        let json = r#"{"x":300,"y":400}"#;
        assert_eq!(parse_bbox_centroid(json), Some((300, 400)));
    }

    #[test]
    fn array_four_elements_returns_centroid() {
        // [x1, y1, x2, y2] = [100, 200, 150, 230] → centroid = (125, 215)
        let json = "[100, 200, 150, 230]";
        assert_eq!(parse_bbox_centroid(json), Some((125, 215)));
    }

    #[test]
    fn array_two_elements_returns_point() {
        let json = "[50, 75]";
        assert_eq!(parse_bbox_centroid(json), Some((50, 75)));
    }

    #[test]
    fn short_array_returns_none() {
        let json = "[42]";
        assert_eq!(parse_bbox_centroid(json), None);

        let json_empty = "[]";
        assert_eq!(parse_bbox_centroid(json_empty), None);
    }

    #[test]
    fn malformed_json_returns_none() {
        assert_eq!(parse_bbox_centroid("not json"), None);
        assert_eq!(parse_bbox_centroid("{broken"), None);
        assert_eq!(parse_bbox_centroid(""), None);
    }

    #[test]
    fn negative_coordinates_saturate_to_zero() {
        // f64 → u32 cast: negative values saturate to 0 in Rust.
        // x=-10 → 0u32, y=-20 → 0u32; w and h default to 0 when absent.
        let json = r#"{"x":-10,"y":-20}"#;
        let result = parse_bbox_centroid(json);
        assert_eq!(result, Some((0, 0)));

        // Negative coordinates in array form
        let arr_json = "[-5, -15, -1, -3]";
        let (cx, cy) = parse_bbox_centroid(arr_json).unwrap();
        assert_eq!(cx, 0);
        assert_eq!(cy, 0);
    }
}
