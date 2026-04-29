use oneshim_core::error::CoreError;
use oneshim_core::models::annotation::{AnnotationType, FrameAnnotation};
use oneshim_core::ports::annotation_storage::AnnotationStorage;

use super::SqliteStorage;

impl AnnotationStorage for SqliteStorage {
    /// List all annotations attached to a given frame, ordered by creation time.
    fn list_annotations(&self, frame_id: i64) -> Result<Vec<FrameAnnotation>, CoreError> {
        let conn = self.conn.lock().map_err(|e| CoreError::Storage {
            code: oneshim_core::error_codes::StorageCode::Failed,
            message: format!("lock: {e}"),
        })?;

        let mut stmt = conn
            .prepare(
                "SELECT annotation_id, frame_id, annotation_type, x, y, width, height, color, text, created_at
                 FROM frame_annotations
                 WHERE frame_id = ?1
                 ORDER BY created_at",
            )
            .map_err(|e| CoreError::Storage { code: oneshim_core::error_codes::StorageCode::Failed, message: format!("prepare: {e}") })?;

        let rows = stmt
            .query_map([frame_id], |row| {
                Ok(AnnotationRow {
                    annotation_id: row.get(0)?,
                    frame_id: row.get(1)?,
                    annotation_type: row.get(2)?,
                    x: row.get(3)?,
                    y: row.get(4)?,
                    width: row.get::<_, f64>(5)? as f32,
                    height: row.get::<_, f64>(6)? as f32,
                    color: row.get(7)?,
                    text: row.get(8)?,
                    created_at: row.get(9)?,
                })
            })
            .map_err(|e| CoreError::Storage {
                code: oneshim_core::error_codes::StorageCode::Failed,
                message: format!("query: {e}"),
            })?;

        let mut result = Vec::new();
        for row in rows {
            let row = row.map_err(|e| CoreError::Storage {
                code: oneshim_core::error_codes::StorageCode::Failed,
                message: format!("row: {e}"),
            })?;
            result.push(row.into_annotation()?);
        }
        Ok(result)
    }

    /// Persist a new annotation to the `frame_annotations` table.
    fn save_annotation(&self, annotation: &FrameAnnotation) -> Result<(), CoreError> {
        let conn = self.conn.lock().map_err(|e| CoreError::Storage {
            code: oneshim_core::error_codes::StorageCode::Failed,
            message: format!("lock: {e}"),
        })?;

        let created_at_str = annotation.created_at.to_rfc3339();

        conn.execute(
            "INSERT INTO frame_annotations
             (annotation_id, frame_id, annotation_type, x, y, width, height, color, text, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            rusqlite::params![
                annotation.annotation_id,
                annotation.frame_id,
                annotation.annotation_type.as_str(),
                annotation.x as f64,
                annotation.y as f64,
                annotation.width as f64,
                annotation.height as f64,
                annotation.color,
                annotation.text,
                created_at_str,
            ],
        )
        .map_err(|e| CoreError::Storage { code: oneshim_core::error_codes::StorageCode::Failed, message: format!("insert: {e}") })?;

        Ok(())
    }

    /// Delete an annotation by ID. No error if the ID does not exist.
    fn delete_annotation(&self, annotation_id: &str) -> Result<(), CoreError> {
        let conn = self.conn.lock().map_err(|e| CoreError::Storage {
            code: oneshim_core::error_codes::StorageCode::Failed,
            message: format!("lock: {e}"),
        })?;

        conn.execute(
            "DELETE FROM frame_annotations WHERE annotation_id = ?1",
            [annotation_id],
        )
        .map_err(|e| CoreError::Storage {
            code: oneshim_core::error_codes::StorageCode::Failed,
            message: format!("delete: {e}"),
        })?;

        Ok(())
    }
}

/// Internal helper struct for reading annotation rows from SQLite.
struct AnnotationRow {
    annotation_id: String,
    frame_id: i64,
    annotation_type: String,
    x: f64,
    y: f64,
    width: f32,
    height: f32,
    color: Option<String>,
    text: Option<String>,
    created_at: String,
}

impl AnnotationRow {
    fn into_annotation(self) -> Result<FrameAnnotation, CoreError> {
        let created_at = chrono::DateTime::parse_from_rfc3339(&self.created_at)
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .map_err(|e| CoreError::Storage {
                code: oneshim_core::error_codes::StorageCode::Failed,
                message: format!("parse created_at: {e}"),
            })?;

        Ok(FrameAnnotation {
            annotation_id: self.annotation_id,
            frame_id: self.frame_id,
            annotation_type: AnnotationType::from_str_lossy(&self.annotation_type),
            x: self.x as f32,
            y: self.y as f32,
            width: self.width,
            height: self.height,
            color: self.color,
            text: self.text,
            created_at,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use oneshim_core::ports::annotation_storage::AnnotationStorage;

    fn test_annotation(id: &str, frame_id: i64, ty: AnnotationType) -> FrameAnnotation {
        FrameAnnotation {
            annotation_id: id.to_string(),
            frame_id,
            annotation_type: ty,
            x: 10.0,
            y: 20.0,
            width: 100.0,
            height: 50.0,
            color: Some("#ff0000".to_string()),
            text: Some("Test note".to_string()),
            created_at: Utc::now(),
        }
    }

    #[test]
    fn annotation_crud_roundtrip() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        // Save annotations
        let ann1 = test_annotation("ann-1", 42, AnnotationType::Highlight);
        let ann2 = test_annotation("ann-2", 42, AnnotationType::Memo);
        AnnotationStorage::save_annotation(&storage, &ann1).unwrap();
        AnnotationStorage::save_annotation(&storage, &ann2).unwrap();

        // List should return both for frame 42
        let list = AnnotationStorage::list_annotations(&storage, 42).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].annotation_id, "ann-1");
        assert_eq!(list[0].annotation_type, AnnotationType::Highlight);
        assert_eq!(list[1].annotation_id, "ann-2");
        assert_eq!(list[1].annotation_type, AnnotationType::Memo);
        assert_eq!(list[0].x, 10.0);
        assert_eq!(list[0].color, Some("#ff0000".to_string()));

        // Delete one
        AnnotationStorage::delete_annotation(&storage, "ann-1").unwrap();

        // List should return only one
        let list = AnnotationStorage::list_annotations(&storage, 42).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].annotation_id, "ann-2");
    }

    #[test]
    fn list_empty_returns_empty() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let list = AnnotationStorage::list_annotations(&storage, 999).unwrap();
        assert!(list.is_empty());
    }

    #[test]
    fn delete_nonexistent_ok() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        // Should not return an error when deleting a non-existent annotation
        AnnotationStorage::delete_annotation(&storage, "nonexistent").unwrap();
    }

    #[test]
    fn annotations_scoped_to_frame_id() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        let ann1 = test_annotation("ann-a", 10, AnnotationType::Highlight);
        let ann2 = test_annotation("ann-b", 20, AnnotationType::Arrow);
        AnnotationStorage::save_annotation(&storage, &ann1).unwrap();
        AnnotationStorage::save_annotation(&storage, &ann2).unwrap();

        // Frame 10 should only have ann-a
        let list_10 = AnnotationStorage::list_annotations(&storage, 10).unwrap();
        assert_eq!(list_10.len(), 1);
        assert_eq!(list_10[0].annotation_id, "ann-a");

        // Frame 20 should only have ann-b
        let list_20 = AnnotationStorage::list_annotations(&storage, 20).unwrap();
        assert_eq!(list_20.len(), 1);
        assert_eq!(list_20[0].annotation_id, "ann-b");
    }

    #[test]
    fn annotation_with_no_optional_fields() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        let ann = FrameAnnotation {
            annotation_id: "ann-minimal".to_string(),
            frame_id: 1,
            annotation_type: AnnotationType::Arrow,
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 0.0,
            color: None,
            text: None,
            created_at: Utc::now(),
        };
        AnnotationStorage::save_annotation(&storage, &ann).unwrap();

        let list = AnnotationStorage::list_annotations(&storage, 1).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].annotation_type, AnnotationType::Arrow);
        assert!(list[0].color.is_none());
        assert!(list[0].text.is_none());
    }
}
