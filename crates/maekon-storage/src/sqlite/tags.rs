use crate::error::StorageError;
use std::collections::HashMap;
use tracing::debug;

use super::{FrameRecord, SqliteStorage, TagRecord};

impl SqliteStorage {
    pub fn get_tag_ids_for_frames(
        &self,
        frame_ids: &[i64],
    ) -> Result<HashMap<i64, Vec<i64>>, StorageError> {
        if frame_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        let placeholders: Vec<String> = frame_ids.iter().map(|_| "?".to_string()).collect();
        let sql = format!(
            "SELECT frame_id, tag_id FROM frame_tags WHERE frame_id IN ({})",
            placeholders.join(",")
        );

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| StorageError::Internal(format!("Failed to prepare query: {e}")))?;

        let params: Vec<Box<dyn rusqlite::types::ToSql>> = frame_ids
            .iter()
            .map(|id| Box::new(*id) as Box<dyn rusqlite::types::ToSql>)
            .collect();
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();

        let rows = stmt
            .query_map(param_refs.as_slice(), |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(|e| StorageError::Internal(format!("Failed to execute query: {e}")))?;

        let mut map: HashMap<i64, Vec<i64>> = HashMap::new();
        for row in rows {
            let (frame_id, tag_id) =
                row.map_err(|e| StorageError::Internal(format!("Failed to read row: {e}")))?;
            map.entry(frame_id).or_default().push(tag_id);
        }

        Ok(map)
    }

    /// # Arguments
    pub fn create_tag(&self, name: &str, color: &str) -> Result<TagRecord, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        conn.execute(
            "INSERT INTO tags (name, color) VALUES (?1, ?2)",
            rusqlite::params![name, color],
        )
        .map_err(|e| StorageError::Internal(format!("Failed to create tag: {e}")))?;

        let tag_id = conn.last_insert_rowid();
        let created_at: String = conn
            .query_row(
                "SELECT created_at FROM tags WHERE id = ?1",
                rusqlite::params![tag_id],
                |row| row.get(0),
            )
            .map_err(|e| StorageError::Internal(format!("Failed to query tag: {e}")))?;

        debug!("create: id={}, name={}", tag_id, name);

        Ok(TagRecord {
            id: tag_id,
            name: name.to_string(),
            color: color.to_string(),
            created_at,
        })
    }

    pub fn get_all_tags(&self) -> Result<Vec<TagRecord>, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        let mut stmt = conn
            .prepare("SELECT id, name, color, created_at FROM tags ORDER BY name")
            .map_err(|e| StorageError::Internal(format!("Failed to prepare query: {e}")))?;

        let tags = stmt
            .query_map([], |row| {
                Ok(TagRecord {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    color: row.get(2)?,
                    created_at: row.get(3)?,
                })
            })
            .map_err(|e| StorageError::Internal(format!("Failed to execute query: {e}")))?
            .filter_map(|r| r.ok())
            .collect();

        Ok(tags)
    }

    pub fn get_tag(&self, tag_id: i64) -> Result<Option<TagRecord>, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        let result = conn.query_row(
            "SELECT id, name, color, created_at FROM tags WHERE id = ?1",
            rusqlite::params![tag_id],
            |row| {
                Ok(TagRecord {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    color: row.get(2)?,
                    created_at: row.get(3)?,
                })
            },
        );

        match result {
            Ok(tag) => Ok(Some(tag)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(StorageError::Internal(format!("Failed to query tag: {e}"))),
        }
    }

    pub fn delete_tag(&self, tag_id: i64) -> Result<bool, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        let deleted = conn
            .execute("DELETE FROM tags WHERE id = ?1", rusqlite::params![tag_id])
            .map_err(|e| StorageError::Internal(format!("Failed to delete tag: {e}")))?;

        debug!("delete: id={}, affected={}", tag_id, deleted);
        Ok(deleted > 0)
    }

    pub fn add_tag_to_frame(&self, frame_id: i64, tag_id: i64) -> Result<(), StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        conn.execute(
            "INSERT OR IGNORE INTO frame_tags (frame_id, tag_id) VALUES (?1, ?2)",
            rusqlite::params![frame_id, tag_id],
        )
        .map_err(|e| StorageError::Internal(format!("Failed to add frame tag: {e}")))?;

        debug!("frame add: frame_id={}, tag_id={}", frame_id, tag_id);
        Ok(())
    }

    pub fn remove_tag_from_frame(&self, frame_id: i64, tag_id: i64) -> Result<bool, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        let deleted = conn
            .execute(
                "DELETE FROM frame_tags WHERE frame_id = ?1 AND tag_id = ?2",
                rusqlite::params![frame_id, tag_id],
            )
            .map_err(|e| StorageError::Internal(format!("Failed to remove frame tag: {e}")))?;

        debug!(
            "frame tag removed: frame_id={}, tag_id={}, affected={}",
            frame_id, tag_id, deleted
        );
        Ok(deleted > 0)
    }

    pub fn get_tags_for_frame(&self, frame_id: i64) -> Result<Vec<TagRecord>, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        let mut stmt = conn
            .prepare(
                "SELECT t.id, t.name, t.color, t.created_at
                 FROM tags t
                 INNER JOIN frame_tags ft ON t.id = ft.tag_id
                 WHERE ft.frame_id = ?1
                 ORDER BY t.name",
            )
            .map_err(|e| StorageError::Internal(format!("Failed to prepare query: {e}")))?;

        let tags = stmt
            .query_map(rusqlite::params![frame_id], |row| {
                Ok(TagRecord {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    color: row.get(2)?,
                    created_at: row.get(3)?,
                })
            })
            .map_err(|e| StorageError::Internal(format!("Failed to execute query: {e}")))?
            .filter_map(|r| r.ok())
            .collect();

        Ok(tags)
    }

    pub fn get_frames_by_tag(
        &self,
        tag_id: i64,
        limit: usize,
    ) -> Result<Vec<FrameRecord>, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        let mut stmt = conn
            .prepare(
                "SELECT f.id, f.timestamp, f.trigger_type, f.app_name, f.window_title,
                        f.importance, f.resolution_w, f.resolution_h, f.file_path, f.ocr_text
                 FROM frames f
                 INNER JOIN frame_tags ft ON f.id = ft.frame_id
                 WHERE ft.tag_id = ?1
                 ORDER BY f.timestamp DESC
                 LIMIT ?2",
            )
            .map_err(|e| StorageError::Internal(format!("Failed to prepare query: {e}")))?;

        let frames = stmt
            .query_map(rusqlite::params![tag_id, limit as i64], |row| {
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

    pub fn update_tag(&self, tag_id: i64, name: &str, color: &str) -> Result<bool, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        let updated = conn
            .execute(
                "UPDATE tags SET name = ?1, color = ?2 WHERE id = ?3",
                rusqlite::params![name, color, tag_id],
            )
            .map_err(|e| StorageError::Internal(format!("Failed to update tag: {e}")))?;

        debug!("update: id={}, affected={}", tag_id, updated);
        Ok(updated > 0)
    }
}

#[cfg(test)]
#[allow(dead_code)]
mod tests {
    //! Inline unit tests for `sqlite::tags`.
    //!
    //! Audit-gated per Phase 5-D8 spec: 7 of 10 pub fn already covered
    //! by sqlite/tests.rs:311-424. This module adds only the 3 genuine
    //! residual gaps:
    //! - update_tag on nonexistent tag_id (returns Ok(false))
    //! - get_tag_ids_for_frames batch lookup (entirely uncovered)
    //! - concurrent UNIQUE(name) race (existing test is sequential)

    use std::sync::Arc;

    use super::SqliteStorage;

    fn open_storage() -> SqliteStorage {
        SqliteStorage::open_in_memory(30).expect("in-memory storage")
    }

    // ── update_tag edge cases ──────────────────────────────────────

    /// Per tags.rs:278 `Ok(updated > 0)` — UPDATE on missing row is
    /// not a SQL error; the port method returns Ok(false).
    #[test]
    fn update_tag_on_nonexistent_returns_ok_false() {
        let storage = open_storage();
        let result = storage.update_tag(99_999, "does-not-matter", "#000000");
        assert!(matches!(result, Ok(false)));
    }

    // ── get_tag_ids_for_frames batch lookup ────────────────────────

    /// Covers the batch method at tags.rs:8-52 which is fully
    /// uncovered by sibling tests.
    #[test]
    fn get_tag_ids_for_frames_happy_path_batch() {
        let storage = open_storage();

        // Seed 2 frames via direct SQL (test has no frame API handy).
        {
            let conn = storage.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO frames (timestamp, trigger_type, app_name, window_title, importance, resolution_w, resolution_h, has_image) \
                 VALUES ('2026-04-18T00:00:00Z', 'manual', 'a', 'a', 0.5, 1920, 1080, 0)",
                [],
            ).unwrap();
            conn.execute(
                "INSERT INTO frames (timestamp, trigger_type, app_name, window_title, importance, resolution_w, resolution_h, has_image) \
                 VALUES ('2026-04-18T00:00:01Z', 'manual', 'b', 'b', 0.5, 1920, 1080, 0)",
                [],
            ).unwrap();
        }

        let tag_a = storage.create_tag("a", "#ff0000").unwrap();
        let tag_b = storage.create_tag("b", "#00ff00").unwrap();

        storage.add_tag_to_frame(1, tag_a.id).unwrap();
        storage.add_tag_to_frame(1, tag_b.id).unwrap();
        storage.add_tag_to_frame(2, tag_b.id).unwrap();

        // Batch lookup for both frames.
        let map = storage.get_tag_ids_for_frames(&[1, 2]).unwrap();
        assert_eq!(map.len(), 2, "both frames present in result");
        assert_eq!(map.get(&1).map(|v| v.len()), Some(2), "frame 1 has 2 tags");
        assert_eq!(map.get(&2).map(|v| v.len()), Some(1), "frame 2 has 1 tag");

        // Empty input short-circuits to empty map.
        let empty = storage.get_tag_ids_for_frames(&[]).unwrap();
        assert!(empty.is_empty());
    }

    // ── lock-contract regression: concurrent UNIQUE race ───────────

    /// UNIQUE(name) constraint at migration/v01_v08.rs:189.
    /// Existing duplicate_tag_name_fails at sqlite/tests.rs:394 is
    /// sequential; this tests multi-thread racing.
    #[test]
    fn concurrent_create_same_name_enforces_uniqueness() {
        let storage = Arc::new(open_storage());
        let mut handles = Vec::new();

        for _ in 0..4 {
            let s = storage.clone();
            handles.push(std::thread::spawn(move || {
                s.create_tag("race-name", "#000000")
            }));
        }

        let mut ok_count = 0;
        let mut err_count = 0;
        for h in handles {
            match h.join().unwrap() {
                Ok(_) => ok_count += 1,
                Err(_) => err_count += 1,
            }
        }

        assert_eq!(ok_count, 1, "exactly one thread wins the UNIQUE race");
        assert_eq!(err_count, 3, "other three get Err from constraint");
    }
}
