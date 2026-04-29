use crate::error::StorageError;
use chrono::{DateTime, Utc};
use oneshim_core::models::context::WindowBounds;
use oneshim_core::models::frame::FrameMetadata;
use oneshim_core::types::TimeWindow;
use tracing::debug;

use super::{FrameRecord, SqliteStorage};

impl SqliteStorage {
    pub fn count_frames_in_range(&self, window: &TimeWindow) -> Result<u64, StorageError> {
        let (from, to) = window.to_sql_pair();
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM frames WHERE timestamp >= ?1 AND timestamp <= ?2",
                rusqlite::params![from, to],
                |row| row.get(0),
            )
            .map_err(|e| StorageError::Internal(format!("Failed to count frames: {e}")))?;

        Ok(count as u64)
    }

    pub fn get_frame_file_path(&self, frame_id: i64) -> Result<Option<String>, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        let result: Result<Option<String>, rusqlite::Error> = conn.query_row(
            "SELECT file_path FROM frames WHERE id = ?1",
            rusqlite::params![frame_id],
            |row| row.get(0),
        );

        match result {
            Ok(path) => Ok(path),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(StorageError::Internal(format!(
                "frame file path query failure: {e}"
            ))),
        }
    }

    /// # Arguments
    pub fn save_frame_metadata(
        &self,
        metadata: &FrameMetadata,
        file_path: Option<&str>,
        ocr_text: Option<&str>,
    ) -> Result<i64, StorageError> {
        self.save_frame_metadata_with_bounds(metadata, file_path, ocr_text, None)
    }

    /// # Arguments
    pub fn save_frame_metadata_with_bounds(
        &self,
        metadata: &FrameMetadata,
        file_path: Option<&str>,
        ocr_text: Option<&str>,
        bounds: Option<&WindowBounds>,
    ) -> Result<i64, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        conn.execute(
            "INSERT INTO frames (timestamp, trigger_type, app_name, window_title, importance, resolution_w, resolution_h, has_image, file_path, ocr_text, window_x, window_y, window_width, window_height)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            rusqlite::params![
                metadata.timestamp.to_rfc3339(),
                metadata.trigger_type,
                metadata.app_name,
                metadata.window_title,
                metadata.importance,
                metadata.resolution.0,
                metadata.resolution.1,
                file_path.is_some(),
                file_path,
                ocr_text,
                bounds.map(|b| b.x),
                bounds.map(|b| b.y),
                bounds.map(|b| b.width as i32),
                bounds.map(|b| b.height as i32),
            ],
        )
        .map_err(|e| StorageError::Internal(format!("Failed to save frame metadata: {e}")))?;

        let frame_id = conn.last_insert_rowid();
        debug!(
            "frame metadata saved: id={}, app={}, file={}",
            frame_id,
            metadata.app_name,
            file_path.unwrap_or("-")
        );

        Ok(frame_id)
    }

    /// # Arguments
    pub fn get_frames(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<FrameRecord>, StorageError> {
        let from_str = from.to_rfc3339();
        let to_str = to.to_rfc3339();

        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        let mut stmt = conn
            .prepare_cached(
                "SELECT id, timestamp, trigger_type, app_name, window_title, importance, resolution_w, resolution_h, file_path, ocr_text
                 FROM frames
                 WHERE timestamp >= ?1 AND timestamp <= ?2
                 ORDER BY timestamp DESC
                 LIMIT ?3",
            )
            .map_err(|e| StorageError::Internal(format!("Failed to prepare query: {e}")))?;

        let frames = stmt
            .query_map(rusqlite::params![from_str, to_str, limit as i64], |row| {
                Ok(FrameRecord {
                    id: row.get(0)?,
                    timestamp: row.get(1)?,
                    trigger_type: row.get(2)?,
                    app_name: row.get(3)?,
                    window_title: row.get(4)?,
                    importance: row.get(5)?,
                    resolution_w: row.get(6)?,
                    resolution_h: row.get(7)?,
                    file_path: row.get(8)?,
                    ocr_text: row.get(9)?,
                })
            })
            .map_err(|e| StorageError::Internal(format!("Failed to execute query: {e}")))?
            .filter_map(|r| r.ok())
            .collect();

        Ok(frames)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};
    use oneshim_core::models::frame::FrameMetadata;

    fn make_metadata() -> FrameMetadata {
        FrameMetadata {
            timestamp: Utc::now(),
            trigger_type: "manual".to_string(),
            app_name: "TestApp".to_string(),
            window_title: "Test Window".to_string(),
            resolution: (1920, 1080),
            importance: 0.5,
        }
    }

    #[test]
    fn count_frames_in_range_empty() {
        let storage = SqliteStorage::open_in_memory(30).expect("open_in_memory failed");
        let from = (Utc::now() - Duration::hours(1)).to_rfc3339();
        let to = (Utc::now() + Duration::hours(1)).to_rfc3339();
        let count = storage
            .count_frames_in_range(
                &TimeWindow::from_rfc3339_pair(&from, &to).expect("trusted test bounds"),
            )
            .expect("count_frames_in_range failed");
        assert_eq!(count, 0);
    }

    #[test]
    fn save_frame_metadata_and_count() {
        let storage = SqliteStorage::open_in_memory(30).expect("open_in_memory failed");
        let meta = make_metadata();
        let frame_id = storage
            .save_frame_metadata(&meta, None, None)
            .expect("save_frame_metadata failed");
        assert!(frame_id > 0);

        let from = (Utc::now() - Duration::hours(1)).to_rfc3339();
        let to = (Utc::now() + Duration::hours(1)).to_rfc3339();
        let count = storage
            .count_frames_in_range(
                &TimeWindow::from_rfc3339_pair(&from, &to).expect("trusted test bounds"),
            )
            .expect("count_frames_in_range failed");
        assert_eq!(count, 1);
    }

    #[test]
    fn get_frame_file_path_nonexistent_returns_none() {
        let storage = SqliteStorage::open_in_memory(30).expect("open_in_memory failed");
        let path = storage
            .get_frame_file_path(99999)
            .expect("get_frame_file_path failed");
        assert!(path.is_none());
    }

    #[test]
    fn save_frame_metadata_with_file_path() {
        let storage = SqliteStorage::open_in_memory(30).expect("open_in_memory failed");
        let meta = make_metadata();
        let frame_id = storage
            .save_frame_metadata(&meta, Some("/tmp/frame.webp"), Some("ocr text"))
            .expect("save_frame_metadata failed");

        let path = storage
            .get_frame_file_path(frame_id)
            .expect("get_frame_file_path failed");
        assert_eq!(path.as_deref(), Some("/tmp/frame.webp"));
    }
}
