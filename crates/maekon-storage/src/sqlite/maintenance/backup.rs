use crate::error::StorageError;

use super::super::{FrameTagLinkRecord, SqliteStorage};

// ADR-026 PR-7: each backup method keeps a synchronous inherent twin (used by
// the storage `#[cfg(test)]` suites + the GDPR erase spawn_blocking path) and
// gains a `*_async` funnel helper, both sharing one `*_inner(&Connection, ...)`
// SQL body. The async `BackupStorage` trait impl delegates to the `*_async`
// helpers; reads use `with_conn_read` (never skipped), writes use the
// `with_conn` write funnel (skipped when `deletion_flag`/`erasing` is set), so
// the #4928 erase barrier is preserved unchanged.
impl SqliteStorage {
    pub fn list_backup_tags(&self) -> Result<Vec<super::super::TagRecord>, StorageError> {
        // 읽기 — read_lock(deletion_flag 무관).
        let read = self.conn.read_lock();
        Self::list_backup_tags_inner(read.conn())
    }

    /// Async `list_backup_tags` over the read funnel (ADR-026 PR-7).
    pub(crate) async fn list_backup_tags_async(
        &self,
    ) -> Result<Vec<super::super::TagRecord>, StorageError> {
        self.with_conn_read(Self::list_backup_tags_inner).await
    }

    fn list_backup_tags_inner(
        conn: &rusqlite::Connection,
    ) -> Result<Vec<super::super::TagRecord>, StorageError> {
        let mut stmt = conn
            .prepare("SELECT id, name, color, created_at FROM tags ORDER BY id")
            .map_err(|e| StorageError::Internal(format!("Failed to prepare query: {e}")))?;

        let rows = stmt
            .query_map([], |row| {
                Ok(super::super::TagRecord {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    color: row.get(2)?,
                    created_at: row.get(3)?,
                })
            })
            .map_err(|e| StorageError::Internal(format!("Failed to execute query: {e}")))?;

        let mut records = Vec::new();
        for row in rows {
            records
                .push(row.map_err(|e| StorageError::Internal(format!("Failed to read row: {e}")))?);
        }
        Ok(records)
    }

    pub fn list_backup_frame_tags(&self) -> Result<Vec<FrameTagLinkRecord>, StorageError> {
        // 읽기 — read_lock(deletion_flag 무관).
        let read = self.conn.read_lock();
        Self::list_backup_frame_tags_inner(read.conn())
    }

    /// Async `list_backup_frame_tags` over the read funnel (ADR-026 PR-7).
    pub(crate) async fn list_backup_frame_tags_async(
        &self,
    ) -> Result<Vec<FrameTagLinkRecord>, StorageError> {
        self.with_conn_read(Self::list_backup_frame_tags_inner)
            .await
    }

    fn list_backup_frame_tags_inner(
        conn: &rusqlite::Connection,
    ) -> Result<Vec<FrameTagLinkRecord>, StorageError> {
        let mut stmt = conn
            .prepare(
                "SELECT frame_id, tag_id, created_at FROM frame_tags ORDER BY frame_id, tag_id",
            )
            .map_err(|e| StorageError::Internal(format!("Failed to prepare query: {e}")))?;

        let rows = stmt
            .query_map([], |row| {
                Ok(FrameTagLinkRecord {
                    frame_id: row.get(0)?,
                    tag_id: row.get(1)?,
                    created_at: row.get(2)?,
                })
            })
            .map_err(|e| StorageError::Internal(format!("Failed to execute query: {e}")))?;

        let mut records = Vec::new();
        for row in rows {
            records
                .push(row.map_err(|e| StorageError::Internal(format!("Failed to read row: {e}")))?);
        }
        Ok(records)
    }

    pub fn upsert_backup_tag(
        &self,
        id: i64,
        name: &str,
        color: &str,
        created_at: &str,
    ) -> Result<(), StorageError> {
        // 쓰기 — write_lock(deletion_flag set 시 스킵, tags ∈ ALL_TABLES).
        self.conn.write_lock().run((), |conn| {
            Self::upsert_backup_tag_inner(conn, id, name, color, created_at)
        })
    }

    /// Async `upsert_backup_tag` over the write funnel (ADR-026 PR-7).
    pub(crate) async fn upsert_backup_tag_async(
        &self,
        id: i64,
        name: String,
        color: String,
        created_at: String,
    ) -> Result<(), StorageError> {
        // Move owned data into the Send + 'static closure (no borrowed &str).
        self.with_conn(move |conn| {
            Self::upsert_backup_tag_inner(conn, id, &name, &color, &created_at)
        })
        .await
    }

    fn upsert_backup_tag_inner(
        conn: &rusqlite::Connection,
        id: i64,
        name: &str,
        color: &str,
        created_at: &str,
    ) -> Result<(), StorageError> {
        conn.execute(
            "INSERT OR IGNORE INTO tags (id, name, color, created_at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![id, name, color, created_at],
        )
        .map_err(|e| StorageError::Internal(format!("Failed to save tag: {e}")))?;

        Ok(())
    }

    pub fn upsert_backup_frame_tag(
        &self,
        frame_id: i64,
        tag_id: i64,
        created_at: &str,
    ) -> Result<(), StorageError> {
        // 쓰기 — write_lock(deletion_flag set 시 스킵, frame_tags ∈ ALL_TABLES).
        self.conn.write_lock().run((), |conn| {
            Self::upsert_backup_frame_tag_inner(conn, frame_id, tag_id, created_at)
        })
    }

    /// Async `upsert_backup_frame_tag` over the write funnel (ADR-026 PR-7).
    pub(crate) async fn upsert_backup_frame_tag_async(
        &self,
        frame_id: i64,
        tag_id: i64,
        created_at: String,
    ) -> Result<(), StorageError> {
        self.with_conn(move |conn| {
            Self::upsert_backup_frame_tag_inner(conn, frame_id, tag_id, &created_at)
        })
        .await
    }

    fn upsert_backup_frame_tag_inner(
        conn: &rusqlite::Connection,
        frame_id: i64,
        tag_id: i64,
        created_at: &str,
    ) -> Result<(), StorageError> {
        conn.execute(
            "INSERT OR IGNORE INTO frame_tags (frame_id, tag_id, created_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![frame_id, tag_id, created_at],
        )
        .map_err(|e| StorageError::Internal(format!("frame-Failed to save tag: {e}")))?;

        Ok(())
    }

    pub fn upsert_backup_event(
        &self,
        event_id: &str,
        event_type: &str,
        timestamp: &str,
        app_name: Option<&str>,
        window_title: Option<&str>,
    ) -> Result<(), StorageError> {
        // 쓰기 — write_lock(deletion_flag set 시 스킵, events ∈ ALL_TABLES).
        self.conn.write_lock().run((), |conn| {
            Self::upsert_backup_event_inner(
                conn,
                event_id,
                event_type,
                timestamp,
                app_name,
                window_title,
            )
        })
    }

    /// Async `upsert_backup_event` over the write funnel (ADR-026 PR-7).
    pub(crate) async fn upsert_backup_event_async(
        &self,
        event_id: String,
        event_type: String,
        timestamp: String,
        app_name: Option<String>,
        window_title: Option<String>,
    ) -> Result<(), StorageError> {
        self.with_conn(move |conn| {
            Self::upsert_backup_event_inner(
                conn,
                &event_id,
                &event_type,
                &timestamp,
                app_name.as_deref(),
                window_title.as_deref(),
            )
        })
        .await
    }

    fn upsert_backup_event_inner(
        conn: &rusqlite::Connection,
        event_id: &str,
        event_type: &str,
        timestamp: &str,
        app_name: Option<&str>,
        window_title: Option<&str>,
    ) -> Result<(), StorageError> {
        let data = serde_json::json!({
            "app_name": app_name,
            "window_title": window_title,
        })
        .to_string();

        conn.execute(
            "INSERT OR IGNORE INTO events (event_id, event_type, timestamp, data)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![event_id, event_type, timestamp, data],
        )
        .map_err(|e| StorageError::Internal(format!("event save failure: {e}")))?;

        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn upsert_backup_frame(
        &self,
        id: i64,
        timestamp: &str,
        trigger_type: &str,
        app_name: &str,
        window_title: &str,
        importance: f32,
        width: i32,
        height: i32,
        ocr_text: Option<&str>,
    ) -> Result<(), StorageError> {
        // 쓰기 — write_lock(deletion_flag set 시 스킵, frames ∈ ALL_TABLES).
        self.conn.write_lock().run((), |conn| {
            Self::upsert_backup_frame_inner(
                conn,
                id,
                timestamp,
                trigger_type,
                app_name,
                window_title,
                importance,
                width,
                height,
                ocr_text,
            )
        })
    }

    /// Async `upsert_backup_frame` over the write funnel (ADR-026 PR-7).
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn upsert_backup_frame_async(
        &self,
        id: i64,
        timestamp: String,
        trigger_type: String,
        app_name: String,
        window_title: String,
        importance: f32,
        width: i32,
        height: i32,
        ocr_text: Option<String>,
    ) -> Result<(), StorageError> {
        self.with_conn(move |conn| {
            Self::upsert_backup_frame_inner(
                conn,
                id,
                &timestamp,
                &trigger_type,
                &app_name,
                &window_title,
                importance,
                width,
                height,
                ocr_text.as_deref(),
            )
        })
        .await
    }

    #[allow(clippy::too_many_arguments)]
    fn upsert_backup_frame_inner(
        conn: &rusqlite::Connection,
        id: i64,
        timestamp: &str,
        trigger_type: &str,
        app_name: &str,
        window_title: &str,
        importance: f32,
        width: i32,
        height: i32,
        ocr_text: Option<&str>,
    ) -> Result<(), StorageError> {
        let exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM frames WHERE id = ?1)",
                rusqlite::params![id],
                |row| row.get(0),
            )
            .unwrap_or(false);

        if !exists {
            conn.execute(
                "INSERT INTO frames (
                    id, timestamp, trigger_type, app_name, window_title,
                    importance, resolution_w, resolution_h, has_image, ocr_text, file_path
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, ?9, NULL)",
                rusqlite::params![
                    id,
                    timestamp,
                    trigger_type,
                    app_name,
                    window_title,
                    importance,
                    width,
                    height,
                    ocr_text,
                ],
            )
            .map_err(|e| StorageError::Internal(format!("frame save failure: {e}")))?;
        }

        Ok(())
    }
}
