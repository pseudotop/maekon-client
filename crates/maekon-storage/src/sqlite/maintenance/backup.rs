use rusqlite::OptionalExtension;

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
        // read — read_lock (independent of deletion_flag).
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
        // read — read_lock (independent of deletion_flag).
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
    ) -> Result<Option<i64>, StorageError> {
        // write — write_lock (skipped when deletion_flag is set; tags ∈ ALL_TABLES).
        // The skip value is `None`, not an id: during an erase nothing is
        // written, and handing the caller an id would have it remap frame_tags
        // onto a row that does not exist.
        self.conn.write_lock().run(None, |conn| {
            Self::upsert_backup_tag_inner(conn, id, name, color, created_at).map(Some)
        })
    }

    /// Async `upsert_backup_tag` over the write funnel (ADR-026 PR-7).
    pub(crate) async fn upsert_backup_tag_async(
        &self,
        id: i64,
        name: String,
        color: String,
        created_at: String,
    ) -> Result<Option<i64>, StorageError> {
        // Move owned data into the Send + 'static closure (no borrowed &str).
        // `with_conn` skips to `T::default()` on erase, which for `Option<i64>`
        // is `None` — the same "not written" signal as the sync twin. (With a
        // bare `i64` it would have been `0`, a live sentinel id in this
        // codebase, silently remapping every relation onto it.)
        self.with_conn(move |conn| {
            Self::upsert_backup_tag_inner(conn, id, &name, &color, &created_at).map(Some)
        })
        .await
    }

    /// Restore one archived tag, returning the id it actually occupies.
    ///
    /// #9700: restore MERGES into the live table — nothing clears `tags` first.
    /// The previous `INSERT OR IGNORE ... ` swallowed both collision kinds (the
    /// `id` primary key and the `name` UNIQUE) and discarded the affected-row
    /// count, so a dropped tag was still counted as restored. `frame_tags` rows
    /// were then restored against the ARCHIVE's id, which — with
    /// `PRAGMA foreign_keys` was believed OFF (it is ON — #9735) — silently produced either a
    /// mis-attached relation (id taken by a different tag) or a dangling one
    /// (name taken under a different id).
    ///
    /// The returned id lets the caller remap `frame_tags` instead:
    /// - name already present → that row IS this tag; return its id
    /// - id free → insert under the archived id
    /// - id taken by a different name → insert under a fresh id
    fn upsert_backup_tag_inner(
        conn: &rusqlite::Connection,
        id: i64,
        name: &str,
        color: &str,
        created_at: &str,
    ) -> Result<i64, StorageError> {
        // Name is the tag's identity across devices; the archived id is not.
        // Exact match — `tags.name` is UNIQUE with no COLLATE, so SQLite treats
        // "Work" and "work" as different tags and so must we.
        let existing: Option<i64> = conn
            .query_row(
                "SELECT id FROM tags WHERE name = ?1",
                rusqlite::params![name],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| StorageError::Internal(format!("Failed to look up tag: {e}")))?;
        if let Some(existing_id) = existing {
            return Ok(existing_id);
        }

        let id_taken: bool = conn
            .query_row(
                "SELECT 1 FROM tags WHERE id = ?1",
                rusqlite::params![id],
                |_| Ok(()),
            )
            .optional()
            .map_err(|e| StorageError::Internal(format!("Failed to look up tag id: {e}")))?
            .is_some();

        if id_taken {
            // Let SQLite assign a free rowid, then report it back so the
            // caller's frame_tags land on this tag rather than the squatter.
            conn.execute(
                "INSERT INTO tags (name, color, created_at) VALUES (?1, ?2, ?3)",
                rusqlite::params![name, color, created_at],
            )
            .map_err(|e| StorageError::Internal(format!("Failed to save tag: {e}")))?;
            return Ok(conn.last_insert_rowid());
        }

        conn.execute(
            "INSERT INTO tags (id, name, color, created_at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![id, name, color, created_at],
        )
        .map_err(|e| StorageError::Internal(format!("Failed to save tag: {e}")))?;
        Ok(id)
    }

    pub fn upsert_backup_frame_tag(
        &self,
        frame_id: i64,
        tag_id: i64,
        created_at: &str,
    ) -> Result<bool, StorageError> {
        // write — write_lock (skipped when deletion_flag is set; frame_tags ∈ ALL_TABLES).
        // Skip value is `false` — during an erase nothing lands, and the caller
        // must not count a write that did not happen.
        self.conn.write_lock().run(false, |conn| {
            Self::upsert_backup_frame_tag_inner(conn, frame_id, tag_id, created_at)
        })
    }

    /// Async `upsert_backup_frame_tag` over the write funnel (ADR-026 PR-7).
    pub(crate) async fn upsert_backup_frame_tag_async(
        &self,
        frame_id: i64,
        tag_id: i64,
        created_at: String,
    ) -> Result<bool, StorageError> {
        // `bool::default()` is `false` — same "nothing landed" signal as the
        // sync twin's explicit skip value.
        self.with_conn(move |conn| {
            Self::upsert_backup_frame_tag_inner(conn, frame_id, tag_id, &created_at)
        })
        .await
    }

    /// Insert one archived relation, returning whether a row actually landed.
    ///
    /// The `WHERE EXISTS` guard is what makes the frame reference safe. The
    /// caller used to check `frame_exists` first and then insert, which is two
    /// separate lock acquisitions with an `.await` between them, so a concurrent
    /// frame delete (retention / app-deletion) could land in the window.
    ///
    /// #9735 corrects what that race actually produced. `frame_tags.frame_id`
    /// does declare `REFERENCES frames(id)`, and the FK is enforced, so the
    /// losing insert did not write a dangling row — it failed:
    /// `FOREIGN KEY constraint failed` (measured). That surfaced as a restore
    /// error on a frame the device legitimately no longer has, which is the
    /// one outcome this method's contract rules out. Guarding inside the
    /// statement makes the check and the write one atomic operation under a
    /// single write lock, turning the race into a clean `false` — and halves
    /// the funnel crossings per relation.
    ///
    /// Returns whether the relation is PRESENT afterwards — `false` means the
    /// frame is not on this device, never that the write failed.
    fn upsert_backup_frame_tag_inner(
        conn: &rusqlite::Connection,
        frame_id: i64,
        tag_id: i64,
        created_at: &str,
    ) -> Result<bool, StorageError> {
        let affected = conn
            .execute(
                "INSERT OR IGNORE INTO frame_tags (frame_id, tag_id, created_at) \
                 SELECT ?1, ?2, ?3 WHERE EXISTS(SELECT 1 FROM frames WHERE id = ?1)",
                rusqlite::params![frame_id, tag_id, created_at],
            )
            .map_err(|e| StorageError::Internal(format!("frame-tag save failure: {e}")))?;
        if affected > 0 {
            return Ok(true);
        }

        // Zero rows is ambiguous: the frame may be missing, OR the relation may
        // already exist and `OR IGNORE` swallowed the conflict. Reporting the
        // latter as "frame not on this device" would make a repeat restore lie
        // about every relation it already holds. Disambiguate under the SAME
        // lock — this runs only on the rare path, so the common case stays one
        // statement.
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM frames WHERE id = ?1)",
            rusqlite::params![frame_id],
            |row| row.get(0),
        )
        .map_err(|e| StorageError::Internal(format!("frame lookup failure: {e}")))
    }

    pub fn upsert_backup_event(
        &self,
        event_id: &str,
        event_type: &str,
        timestamp: &str,
        app_name: Option<&str>,
        window_title: Option<&str>,
    ) -> Result<bool, StorageError> {
        // write — write_lock (skipped when deletion_flag is set; events ∈ ALL_TABLES).
        // #9722: skip value is `false`, so the caller cannot count a write that
        // never happened. Matches the tag/frame/relation paths.
        self.conn.write_lock().run(false, |conn| {
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
    ) -> Result<bool, StorageError> {
        // `bool::default()` is `false` — the same "nothing landed" signal.
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
    ) -> Result<bool, StorageError> {
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

        // Zero affected rows means the event_id was already present — the event
        // IS there either way. Unlike `frame_tags` there is no absent-parent
        // case to disambiguate, so the only `false` is the erase skip above.
        Ok(true)
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
    ) -> Result<Option<i64>, StorageError> {
        // write — write_lock (skipped when deletion_flag is set; frames ∈ ALL_TABLES).
        // Skip value is `None`, not an id — see `upsert_backup_tag`.
        self.conn.write_lock().run(None, |conn| {
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
            .map(Some)
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
    ) -> Result<Option<i64>, StorageError> {
        // `Option<i64>::default()` is `None` — the erase-skip signal, matching
        // the tag path. A bare `i64` would skip to `0`.
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
            .map(Some)
        })
        .await
    }

    /// Restore one archived frame, returning the id it actually occupies.
    ///
    /// #9708, the frame twin of #9700's tag fix. `frames.id` is
    /// `INTEGER PRIMARY KEY AUTOINCREMENT`, so on any device that has been
    /// capturing, ids 1..N are already taken and essentially EVERY archived
    /// frame collides. The previous body silently declined to insert on
    /// collision and returned `Ok(())` regardless, so the caller counted a
    /// dropped frame as restored AND then wrote its `frame_tags` against the
    /// archived id — attaching them to an unrelated local screenshot.
    ///
    /// Relocating is safe here in a way it would not be for a live frame:
    /// restored frames are metadata-only by construction (`has_image = 0`,
    /// `file_path = NULL`, and `FrameBackup` carries no path), so no image file
    /// is keyed to the id.
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
    ) -> Result<i64, StorageError> {
        let id_taken: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM frames WHERE id = ?1)",
                rusqlite::params![id],
                |row| row.get(0),
            )
            .map_err(|e| StorageError::Internal(format!("frame lookup failure: {e}")))?;

        // Unlike tags, frames have no natural identity key (no UNIQUE column),
        // so a collision cannot mean "this is the same frame". Relocate rather
        // than drop: the archived metadata survives and its relations follow.
        if id_taken {
            conn.execute(
                "INSERT INTO frames (
                    timestamp, trigger_type, app_name, window_title,
                    importance, resolution_w, resolution_h, has_image, ocr_text, file_path
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, ?8, NULL)",
                rusqlite::params![
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
            return Ok(conn.last_insert_rowid());
        }

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
        Ok(id)
    }
}
