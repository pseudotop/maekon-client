//! Retroactive per-app data deletion (#8045 B2 — Recall parity).
//!
//! Adding an app to the exclusion list is a pure config write, so any history
//! already recorded for that app used to survive (only full-wipe and date-range
//! deletion primitives existed). This module implements the missing per-app
//! primitive: when an app (or app-name pattern) is newly excluded, its
//! already-stored `frames` (metadata + files), `events`, and the derived
//! `activity_segments` (LLM summaries) + `embedding_vectors` are purged.
//!
//! `activity_segments` and `embedding_vectors` are cross-device-synced tables,
//! so — mirroring the age-based sweeps (#8043) and the range-delete cascade
//! (#8045 B1) — a LOCAL-origin suppression tombstone is captured for each
//! removed row BEFORE the DELETE so a peer that still holds a locally-authored
//! row cannot re-push (resurrect) it on the next sync.

use maekon_core::models::storage_records::AppDeletionCounts;

use crate::error::StorageError;

use super::super::SqliteStorage;

/// SQL-escape a string literal's single quotes so it is safe to embed directly
/// in a SQL string literal (`''` is an escaped single quote). App names come
/// from the local user's own config, but an OS app name can legitimately
/// contain an apostrophe, so this is correctness (not only injection defense).
fn sql_quote(value: &str) -> String {
    value.replace('\'', "''")
}

/// Escape the LIKE metacharacters (`%`, `_`, `\`) in a literal fragment, using
/// `\` as the `ESCAPE` character, then SQL-escape single quotes. Used for the
/// literal parts of a glob→LIKE translation so an app name containing `%`/`_`
/// is matched literally rather than as a wildcard.
fn like_escape(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    escaped.replace('\'', "''")
}

/// One glob pattern translated into a LIKE body (with the surrounding `%`
/// already applied). Returns the SQL body **without** the column name, e.g.
/// `LIKE '%bank%' ESCAPE '\'` or `= 'exact' COLLATE NOCASE`.
enum MatchBody {
    /// Case-insensitive exact equality.
    Equals(String),
    /// A `LIKE` body with the `%` wildcards already placed.
    Like(String),
}

/// Translate one capture-time glob pattern (`*k*` / `*s` / `p*` / exact) into a
/// [`MatchBody`], mirroring `maekon_vision::privacy::matches_exclusion_pattern`.
fn glob_to_body(pattern: &str) -> MatchBody {
    if let Some(rest) = pattern.strip_prefix('*') {
        if let Some(keyword) = rest.strip_suffix('*') {
            MatchBody::Like(format!("LIKE '%{}%' ESCAPE '\\'", like_escape(keyword)))
        } else {
            MatchBody::Like(format!("LIKE '%{}' ESCAPE '\\'", like_escape(rest)))
        }
    } else if let Some(prefix) = pattern.strip_suffix('*') {
        MatchBody::Like(format!("LIKE '{}%' ESCAPE '\\'", like_escape(prefix)))
    } else {
        MatchBody::Equals(format!("= '{}' COLLATE NOCASE", sql_quote(pattern)))
    }
}

/// Build a boolean predicate matching a DIRECT text column (`frames.app_name`
/// or the `json_extract(data,'$.app_name')` of `events`) against the exact
/// `names` (case-insensitive equality) and glob `patterns`. Returns `None` when
/// there is nothing to match (both lists empty), so the caller skips the table.
fn direct_column_predicate(
    col_expr: &str,
    names: &[String],
    patterns: &[String],
) -> Option<String> {
    let mut clauses: Vec<String> = Vec::new();
    for name in names {
        clauses.push(format!("{col_expr} = '{}' COLLATE NOCASE", sql_quote(name)));
    }
    for pattern in patterns {
        match glob_to_body(pattern) {
            MatchBody::Equals(body) => clauses.push(format!("{col_expr} {body}")),
            MatchBody::Like(body) => clauses.push(format!("{col_expr} {body}")),
        }
    }
    if clauses.is_empty() {
        None
    } else {
        Some(format!("({})", clauses.join(" OR ")))
    }
}

/// Build a predicate matching an `activity_segments` row whose `app_breakdown`
/// JSON references one of the apps. `app_breakdown` is a `{"AppName": seconds}`
/// map whose only text tokens are the app-name keys (values are numbers), so a
/// keyword `LIKE` on the lower-cased column matches only when some KEY contains
/// the keyword. Exact names use the quote-bounded `%"name"%` form for a precise
/// whole-key match; glob patterns match the key with the appropriate quote
/// anchor.
fn breakdown_predicate(names: &[String], patterns: &[String]) -> Option<String> {
    let col = "LOWER(app_breakdown)";
    let mut clauses: Vec<String> = Vec::new();
    for name in names {
        // Whole-key match: the key is stored as `"AppName"`, so bound with quotes.
        clauses.push(format!(
            "{col} LIKE '%\"{}\"%' ESCAPE '\\'",
            like_escape(&name.to_lowercase())
        ));
    }
    for pattern in patterns {
        let p = pattern.to_lowercase();
        let clause = if let Some(rest) = p.strip_prefix('*') {
            if let Some(keyword) = rest.strip_suffix('*') {
                // key contains keyword
                format!("{col} LIKE '%{}%' ESCAPE '\\'", like_escape(keyword))
            } else {
                // key ends with `rest` (keyword immediately before the closing quote)
                format!("{col} LIKE '%{}\"%' ESCAPE '\\'", like_escape(rest))
            }
        } else if let Some(prefix) = p.strip_suffix('*') {
            // key starts with `prefix` (opening quote immediately before keyword)
            format!("{col} LIKE '%\"{}%' ESCAPE '\\'", like_escape(prefix))
        } else {
            format!("{col} LIKE '%\"{}\"%' ESCAPE '\\'", like_escape(&p))
        };
        clauses.push(clause);
    }
    if clauses.is_empty() {
        None
    } else {
        Some(format!("({})", clauses.join(" OR ")))
    }
}

impl SqliteStorage {
    /// Async `delete_data_for_apps` over the write funnel (skips when
    /// `deletion_flag` is set — harmless, since a full erase already wipes every
    /// table). Mirrors `delete_data_in_range_async`.
    pub(crate) async fn delete_data_for_apps_async(
        &self,
        app_names: &[String],
        app_patterns: &[String],
    ) -> Result<(Vec<String>, AppDeletionCounts), StorageError> {
        let names: Vec<String> = app_names.to_vec();
        let patterns: Vec<String> = app_patterns.to_vec();
        self.with_conn_mut(move |conn| Self::delete_data_for_apps_inner(conn, &names, &patterns))
            .await
    }

    /// Delete every locally-stored row attributable to the given apps/patterns
    /// inside a single transaction, returning the deleted frames' on-disk file
    /// paths (for best-effort file removal by the caller) + per-table counts.
    fn delete_data_for_apps_inner(
        conn: &mut rusqlite::Connection,
        names: &[String],
        patterns: &[String],
    ) -> Result<(Vec<String>, AppDeletionCounts), StorageError> {
        let mut counts = AppDeletionCounts::default();
        let mut file_paths: Vec<String> = Vec::new();

        let frames_pred = direct_column_predicate("app_name", names, patterns);
        let events_pred =
            direct_column_predicate("json_extract(data, '$.app_name')", names, patterns);
        let seg_pred = breakdown_predicate(names, patterns);

        // Nothing to match (empty lists) → no-op.
        if frames_pred.is_none() && events_pred.is_none() && seg_pred.is_none() {
            return Ok((file_paths, counts));
        }

        let tx = conn
            .transaction()
            .map_err(|e| StorageError::Internal(format!("Failed to begin transaction: {e}")))?;

        // ── frames: collect file paths (before delete), then delete metadata ──
        if let Some(ref pred) = frames_pred {
            {
                let mut stmt = tx
                    .prepare(&format!(
                        "SELECT file_path FROM frames WHERE {pred} AND file_path IS NOT NULL"
                    ))
                    .map_err(|e| {
                        StorageError::Internal(format!("prepare app frame-path query: {e}"))
                    })?;
                let rows = stmt
                    .query_map([], |row| row.get::<_, String>(0))
                    .map_err(|e| StorageError::Internal(format!("query app frame paths: {e}")))?;
                for row in rows {
                    file_paths.push(row.map_err(|e| {
                        StorageError::Internal(format!("read app frame path: {e}"))
                    })?);
                }
            }
            // #9721: `frame_annotations` has no FK, so the engine cannot
            // cascade it. This path is a data-minimization action — "delete
            // everything for this app" — so leaving those rows behind kept the
            // user's own memo text past the deletion.
            super::frame_dependents::delete_frame_dependents_matching(&tx, pred)?;

            counts.frames_deleted = tx
                .execute(&format!("DELETE FROM frames WHERE {pred}"), [])
                .map_err(|e| StorageError::Internal(format!("app frame delete failure: {e}")))?
                as u64;
        }

        // ── events: match the app_name inside the event JSON payload ──
        if let Some(ref pred) = events_pred {
            counts.events_deleted = tx
                .execute(&format!("DELETE FROM events WHERE {pred}"), [])
                .map_err(|e| StorageError::Internal(format!("app event delete failure: {e}")))?
                as u64;
        }

        // ── derived cascade: activity_segments + their embeddings ──
        if let Some(ref pred) = seg_pred {
            // Capture LOCAL-origin suppression tombstones BEFORE deleting the
            // synced rows (peer-resurrection guard, #8043 discipline). `None`
            // (sync never ran) → skip capture (a never-synced row cannot be
            // resurrected).
            let local_device = crate::sync_retention_tombstone::local_device_id(&tx);
            if let Some(local) = local_device.as_deref() {
                crate::sync_retention_tombstone::capture_local_origin_retention_tombstones(
                    &tx,
                    "activity_segments",
                    pred,
                    local,
                )?;
            }
            // Embeddings inherit the segment match via their `segment_id` link.
            // The subquery must run while the segments are still LIVE, so both the
            // embedding tombstone capture AND the embedding DELETE run BEFORE the
            // segment DELETE below.
            let emb_pred = format!("segment_id IN (SELECT id FROM activity_segments WHERE {pred})");
            if let Some(local) = local_device.as_deref() {
                crate::sync_retention_tombstone::capture_local_origin_retention_tombstones(
                    &tx,
                    "embedding_vectors",
                    &emb_pred,
                    local,
                )?;
            }
            counts.embedding_vectors_deleted = tx
                .execute(
                    &format!("DELETE FROM embedding_vectors WHERE {emb_pred}"),
                    [],
                )
                .map_err(|e| StorageError::Internal(format!("app embedding delete failure: {e}")))?
                as u64;
            counts.activity_segments_deleted = tx
                .execute(&format!("DELETE FROM activity_segments WHERE {pred}"), [])
                .map_err(|e| StorageError::Internal(format!("app segment delete failure: {e}")))?
                as u64;
        }

        tx.commit()
            .map_err(|e| StorageError::Internal(format!("Failed to commit app deletion: {e}")))?;

        Ok((file_paths, counts))
    }

    /// Synchronous test entry point that runs `delete_data_for_apps_inner` over
    /// the same `write_lock` funnel the async production path uses, without a
    /// tokio runtime.
    #[cfg(test)]
    pub(crate) fn delete_data_for_apps_inner_for_test(
        &self,
        names: &[String],
        patterns: &[String],
    ) -> Result<(Vec<String>, AppDeletionCounts), StorageError> {
        let names = names.to_vec();
        let patterns = patterns.to_vec();
        self.conn
            .write_lock()
            .run_mut((Vec::new(), AppDeletionCounts::default()), move |conn| {
                Self::delete_data_for_apps_inner(conn, &names, &patterns)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::SqliteStorage;

    fn count(storage: &SqliteStorage, sql: &str) -> i64 {
        let conn = storage.connection_arc();
        let guard = conn.test_lock();
        guard.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    /// Seed frames/events/segments/embeddings for two apps: "Banking" (to be
    /// excluded) and "Editor" (must survive), all local-origin.
    fn seed(storage: &SqliteStorage, local: &str) {
        let conn = storage.connection_arc();
        let guard = conn.test_lock();
        // frames (clean app_name column + file_path)
        for (app, path) in [("Banking", "b/1.webp"), ("Editor", "e/1.webp")] {
            guard
                .execute(
                    "INSERT INTO frames (timestamp, trigger_type, app_name, window_title, \
                     importance, resolution_w, resolution_h, has_image, file_path) \
                     VALUES ('2026-01-01T00:00:00+00:00', 'smart', ?1, 'w', 0.5, 1, 1, 1, ?2)",
                    rusqlite::params![app, path],
                )
                .unwrap();
        }
        // events (app_name inside the JSON payload)
        for app in ["Banking", "Editor"] {
            guard
                .execute(
                    "INSERT INTO events (event_id, event_type, timestamp, data) \
                     VALUES (?1, 'Context', '2026-01-01T00:00:00+00:00', ?2)",
                    rusqlite::params![
                        format!("evt-{app}"),
                        serde_json::json!({ "app_name": app }).to_string()
                    ],
                )
                .unwrap();
        }
        // activity_segments (app in app_breakdown JSON keys) + embeddings
        for (id, breakdown) in [
            ("seg-bank", serde_json::json!({ "Banking": 30 })),
            ("seg-edit", serde_json::json!({ "Editor": 30 })),
        ] {
            guard
                .execute(
                    "INSERT INTO activity_segments (id, start_time, end_time, duration_secs, \
                     trigger_reason, dominant_category, app_breakdown, hlc_wall_ms, hlc_counter, \
                     origin_device_id) VALUES (?1, '2026-01-01T00:00:00+00:00', \
                     '2026-01-01T00:01:00+00:00', 60, 'timer', 'Finance', ?2, 100, 1, ?3)",
                    rusqlite::params![id, breakdown.to_string(), local],
                )
                .unwrap();
            guard
                .execute(
                    "INSERT INTO embedding_vectors (segment_id, content_type, original_text, \
                     vector, model_id, timestamp, hlc_wall_ms, hlc_counter, origin_device_id) \
                     VALUES (?1, 'screen', 'txt', x'0102', 'm1', '2026-01-01T00:00:00+00:00', \
                     200, 2, ?2)",
                    rusqlite::params![id, local],
                )
                .unwrap();
        }
    }

    /// #9721: "delete everything for this app" is a data-minimization action.
    /// `frame_annotations` declares no FK, so SQLite could not cascade it and
    /// the user's own memo text survived the deletion. (`frame_tags` and
    /// `interruptions` DO have FKs and are handled by the engine — they are
    /// asserted here as a scoping check, not as a regression guard.)
    #[test]
    fn deleting_an_apps_frames_removes_the_annotations_the_fk_engine_cannot() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let (local, _) = storage.ensure_device_identity("Local").unwrap();
        seed(&storage, &local);

        // Attach dependents to the Banking frame that the deletion targets.
        let banking_frame: i64 = {
            let guard = storage.conn.test_lock();
            let id = guard
                .query_row(
                    "SELECT id FROM frames WHERE app_name = 'Banking'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            guard
                .execute(
                    "INSERT INTO tags (id, name, color, created_at)                      VALUES (900, 'private', '#ef4444', '2026-01-01T00:00:00Z')",
                    [],
                )
                .unwrap();
            guard
                .execute(
                    "INSERT INTO frame_tags (frame_id, tag_id, created_at)                      VALUES (?1, 900, '2026-01-01T00:00:00Z')",
                    rusqlite::params![id],
                )
                .unwrap();
            guard
                .execute(
                    "INSERT INTO frame_annotations (annotation_id, frame_id, annotation_type, x, y, text, created_at)                      VALUES ('ann-bank', ?1, 'memo', 0.1, 0.1, 'account number', '2026-01-01T00:00:00Z')",
                    rusqlite::params![id],
                )
                .unwrap();
            guard
                .execute(
                    "INSERT INTO interruptions (interrupted_at, from_app, from_category, to_app, to_category, snapshot_frame_id)                      VALUES ('2026-01-01T00:00:00Z', 'Banking', 'fin', 'Slack', 'chat', ?1)",
                    rusqlite::params![id],
                )
                .unwrap();
            id
        };

        // The OTHER app's frame keeps its annotation — without this the
        // assertions below pass even if the cleanup wiped the table wholesale.
        let editor_frame: i64 = {
            let guard = storage.conn.test_lock();
            let id = guard
                .query_row(
                    "SELECT id FROM frames WHERE app_name != 'Banking'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            guard
                .execute(
                    "INSERT INTO frame_annotations (annotation_id, frame_id, annotation_type, x, y, text, created_at) \
                     VALUES ('ann-editor', ?1, 'memo', 0.2, 0.2, 'keep me', '2026-01-01T00:00:00Z')",
                    rusqlite::params![id],
                )
                .unwrap();
            id
        };

        storage
            .delete_data_for_apps_inner_for_test(&["Banking".to_string()], &[])
            .unwrap();

        let guard = storage.conn.test_lock();
        let relations: i64 = guard
            .query_row(
                "SELECT COUNT(*) FROM frame_tags WHERE frame_id = ?1",
                rusqlite::params![banking_frame],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(relations, 0, "CASCADE: the relation must go with its frame");

        let annotations: i64 = guard
            .query_row(
                "SELECT COUNT(*) FROM frame_annotations WHERE frame_id = ?1",
                rusqlite::params![banking_frame],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            annotations, 0,
            "the user's memo must not survive a deletion of the app it belonged to"
        );

        let dangling: i64 = guard
            .query_row(
                "SELECT COUNT(*) FROM interruptions WHERE snapshot_frame_id = ?1",
                rusqlite::params![banking_frame],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            dangling, 0,
            "SET NULL: the snapshot reference must be cleared"
        );

        let survivor: i64 = guard
            .query_row(
                "SELECT COUNT(*) FROM frame_annotations WHERE frame_id = ?1",
                rusqlite::params![editor_frame],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            survivor, 1,
            "another app's annotation must survive — the cleanup must be scoped"
        );
    }

    #[test]
    fn deletes_exact_app_and_cascades_to_derivatives_with_tombstones() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let (local, _) = storage.ensure_device_identity("Local").unwrap();
        seed(&storage, &local);

        let (paths, counts) = storage
            .delete_data_for_apps_inner_for_test(&["Banking".to_string()], &[])
            .unwrap();

        assert_eq!(counts.frames_deleted, 1);
        assert_eq!(counts.events_deleted, 1);
        assert_eq!(counts.activity_segments_deleted, 1);
        assert_eq!(counts.embedding_vectors_deleted, 1);
        assert_eq!(paths, vec!["b/1.webp".to_string()]);

        // The excluded app's rows are gone; the other app's rows survive.
        assert_eq!(
            count(
                &storage,
                "SELECT COUNT(*) FROM frames WHERE app_name = 'Editor'"
            ),
            1
        );
        assert_eq!(
            count(
                &storage,
                "SELECT COUNT(*) FROM frames WHERE app_name = 'Banking'"
            ),
            0
        );
        assert_eq!(
            count(
                &storage,
                "SELECT COUNT(*) FROM activity_segments WHERE id = 'seg-edit'"
            ),
            1
        );
        assert_eq!(
            count(
                &storage,
                "SELECT COUNT(*) FROM embedding_vectors WHERE segment_id = 'seg-edit'"
            ),
            1
        );

        // Synced derived rows leave suppression tombstones; the surviving app's
        // rows do NOT.
        assert_eq!(
            count(
                &storage,
                "SELECT COUNT(*) FROM sync_tombstones \
                 WHERE table_name = 'activity_segments' AND row_id = 'seg-bank'"
            ),
            1
        );
        let emb_key = format!("seg-bank{}m1", '\u{1f}');
        let emb_tomb = {
            let conn = storage.connection_arc();
            let guard = conn.test_lock();
            guard
                .query_row(
                    "SELECT COUNT(*) FROM sync_tombstones \
                     WHERE table_name = 'embedding_vectors' AND row_id = ?1",
                    rusqlite::params![emb_key],
                    |r| r.get::<_, i64>(0),
                )
                .unwrap()
        };
        assert_eq!(emb_tomb, 1);
    }

    #[test]
    fn matches_glob_pattern_case_insensitively() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let (local, _) = storage.ensure_device_identity("Local").unwrap();
        seed(&storage, &local);

        // `*bank*` pattern matches "Banking" case-insensitively.
        let (_, counts) = storage
            .delete_data_for_apps_inner_for_test(&[], &["*bank*".to_string()])
            .unwrap();

        assert_eq!(counts.frames_deleted, 1, "pattern must match Banking frame");
        assert_eq!(counts.events_deleted, 1);
        assert_eq!(counts.activity_segments_deleted, 1);
        assert_eq!(
            count(
                &storage,
                "SELECT COUNT(*) FROM frames WHERE app_name = 'Editor'"
            ),
            1
        );
    }

    #[test]
    fn empty_lists_delete_nothing() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let (local, _) = storage.ensure_device_identity("Local").unwrap();
        seed(&storage, &local);

        let (paths, counts) = storage
            .delete_data_for_apps_inner_for_test(&[], &[])
            .unwrap();
        assert_eq!(counts.total(), 0);
        assert!(paths.is_empty());
        assert_eq!(
            count(&storage, "SELECT COUNT(*) FROM frames"),
            2,
            "no frame deleted"
        );
    }

    #[test]
    fn app_name_with_apostrophe_is_escaped_not_injected() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let (_local, _) = storage.ensure_device_identity("Local").unwrap();
        {
            let conn = storage.connection_arc();
            let guard = conn.test_lock();
            guard
                .execute(
                    "INSERT INTO frames (timestamp, trigger_type, app_name, window_title, \
                     importance, resolution_w, resolution_h, has_image, file_path) \
                     VALUES ('2026-01-01T00:00:00+00:00', 'smart', ?1, 'w', 0.5, 1, 1, 1, 'p')",
                    rusqlite::params!["Bob's Player"],
                )
                .unwrap();
        }
        let (_, counts) = storage
            .delete_data_for_apps_inner_for_test(&["Bob's Player".to_string()], &[])
            .unwrap();
        assert_eq!(
            counts.frames_deleted, 1,
            "apostrophe app name must match, not error"
        );
    }
}
