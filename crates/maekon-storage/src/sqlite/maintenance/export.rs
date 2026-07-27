use crate::error::StorageError;
use maekon_core::ports::web_storage::PersonalDataTableExport;

use super::super::{
    EventExportRecord, FrameExportRecord, MetricExportRecord, SearchEventRow, SearchFrameRow,
    SqliteStorage,
};

/// Personal-data tables covered by the GDPR Art.20 (data portability) export
/// (#8056 P2-3). These are exactly the tables the scoped `/export/*` and
/// `/backup` surfaces omit — `events`/`frames`/`tags`/`metrics` already have
/// dedicated export routes and are intentionally not duplicated here. The list
/// is a fixed, code-defined allowlist (never user input), so interpolating a
/// name into `SELECT * FROM {table}` carries no injection risk.
const PERSONAL_DATA_EXPORT_TABLES: &[&str] = &[
    "activity_segments",
    "suggestions",
    "local_suggestions",
    "coaching_events",
    "coaching_effectiveness",
    "habit_streaks",
    "regime_goals",
    "memory_claims",
    "memory_edges",
    "frame_annotations",
    "ai_sessions",
    "ai_conversation_messages",
    "focus_metrics",
    "work_sessions",
    "interruptions",
    "daily_digests",
    "weekly_digests",
    "session_stats",
    // V49 (#8577, ADR-028): durable task lifecycle. Personal-data export includes
    // all task tables through the existing masking path; provenance is minimized
    // and never reconstructs unavailable raw source data.
    "task_candidates",
    "task_source_refs",
    "todo_items",
    "todo_blockers",
    "task_transition_receipts",
    // V51 (#8587, ADR-030 §12 + Amendment M4): work-context ledger. Art.20 export
    // includes envelopes, projections, conflicts, cursors, and tombstone
    // explanations through the existing masking path. `sanitized_title` /
    // `sanitized_summary` (projections) are masked by the web export handler's
    // MASKED_COLUMNS — a new plane absent from BOTH lists silently breaks GDPR
    // export/masking (M4). `work_context_raw_blobs` is DELIBERATELY EXCLUDED: a
    // still-retained raw blob is exportable only through the reauthenticated
    // encrypted-attachment path, never a normal JSON DTO (§12). `access_epochs`
    // is an internal counter, not personal data.
    "work_context_envelopes",
    "work_context_projections",
    "work_context_conflicts",
    "work_context_cursors",
    "work_context_tombstones",
    // V52 (#8588, ADR-029): the user's installed Skill Pack inventory and their
    // explicit activation choice are their data and export with the rest. The
    // paired `skill_pack_activation_audit` is a retained processing record and is
    // exported through the audit route, not here.
    //
    // These tables add no free-text content column, so `MASKED_COLUMNS` in
    // maekon-web's full_export needs no new entry: every column is an identifier,
    // a bounded enum, a digest, or a timestamp.
    "skill_pack_catalog",
    "skill_pack_activation",
];

/// Convert a single SQLite value to JSON. BLOBs are summarized (byte length)
/// rather than embedded so the portability archive stays text/JSON.
fn value_ref_to_json(value: rusqlite::types::ValueRef<'_>) -> serde_json::Value {
    use rusqlite::types::ValueRef;
    match value {
        ValueRef::Null => serde_json::Value::Null,
        ValueRef::Integer(n) => serde_json::Value::from(n),
        ValueRef::Real(f) => serde_json::json!(f),
        ValueRef::Text(t) => serde_json::Value::from(String::from_utf8_lossy(t).into_owned()),
        ValueRef::Blob(b) => serde_json::Value::from(format!("<binary {} bytes>", b.len())),
    }
}

/// Export query for a personal-data table. Retracted memory claims remain in
/// the local graph for lifecycle transparency, but they must not re-enter a
/// portable user-data archive after the user has withdrawn them. Edges that
/// reference those claims are excluded as well so the export cannot preserve
/// a dangling identifier or provenance path back to a retracted belief.
fn personal_data_export_query(table: &str) -> String {
    match table {
        "memory_claims" => "SELECT * FROM memory_claims WHERE status != 'retracted'".to_string(),
        "memory_edges" => "SELECT memory_edges.* FROM memory_edges
             WHERE NOT EXISTS (
                 SELECT 1 FROM memory_claims AS retracted
                 WHERE retracted.status = 'retracted'
                   AND (retracted.claim_id = memory_edges.src_id
                        OR retracted.claim_id = memory_edges.dst_id)
             )"
        .to_string(),
        _ => format!("SELECT * FROM {table}"),
    }
}

impl SqliteStorage {
    pub fn list_event_exports(
        &self,
        from: &str,
        to: &str,
    ) -> Result<Vec<EventExportRecord>, StorageError> {
        // read — read_lock (independent of deletion_flag).
        let read = self.conn.read_lock();
        Self::list_event_exports_inner(read.conn(), from, to)
    }

    /// Async `list_event_exports` over the read funnel (ADR-026 PR-7).
    pub(crate) async fn list_event_exports_async(
        &self,
        from: &str,
        to: &str,
    ) -> Result<Vec<EventExportRecord>, StorageError> {
        // owned move into the Send + 'static closure (no borrowed &str).
        let from = from.to_owned();
        let to = to.to_owned();
        self.with_conn_read(move |conn| Self::list_event_exports_inner(conn, &from, &to))
            .await
    }

    fn list_event_exports_inner(
        conn: &rusqlite::Connection,
        from: &str,
        to: &str,
    ) -> Result<Vec<EventExportRecord>, StorageError> {
        let mut stmt = conn
            .prepare(
                "SELECT event_id, event_type, timestamp,
                        json_extract(data, '$.app_name'),
                        json_extract(data, '$.window_title')
                 FROM events
                 WHERE timestamp >= ?1 AND timestamp <= ?2
                 ORDER BY timestamp ASC",
            )
            .map_err(|e| StorageError::Internal(format!("Failed to prepare query: {e}")))?;

        let rows = stmt
            .query_map(rusqlite::params![from, to], |row| {
                Ok(EventExportRecord {
                    event_id: row.get(0)?,
                    event_type: row.get(1)?,
                    timestamp: row.get(2)?,
                    app_name: row.get(3)?,
                    window_title: row.get(4)?,
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

    pub fn list_metric_exports(
        &self,
        from: &str,
        to: &str,
    ) -> Result<Vec<MetricExportRecord>, StorageError> {
        // read — read_lock (independent of deletion_flag).
        let read = self.conn.read_lock();
        Self::list_metric_exports_inner(read.conn(), from, to)
    }

    /// Async `list_metric_exports` over the read funnel (ADR-026 PR-7).
    pub(crate) async fn list_metric_exports_async(
        &self,
        from: &str,
        to: &str,
    ) -> Result<Vec<MetricExportRecord>, StorageError> {
        // owned move into the Send + 'static closure (no borrowed &str).
        let from = from.to_owned();
        let to = to.to_owned();
        self.with_conn_read(move |conn| Self::list_metric_exports_inner(conn, &from, &to))
            .await
    }

    fn list_metric_exports_inner(
        conn: &rusqlite::Connection,
        from: &str,
        to: &str,
    ) -> Result<Vec<MetricExportRecord>, StorageError> {
        let mut stmt = conn
            .prepare(
                "SELECT timestamp, cpu_usage, memory_used, memory_total, disk_used, disk_total,
                        network_upload, network_download
                 FROM system_metrics
                 WHERE timestamp >= ?1 AND timestamp <= ?2
                 ORDER BY timestamp ASC",
            )
            .map_err(|e| StorageError::Internal(format!("Failed to prepare query: {e}")))?;

        let rows = stmt
            .query_map(rusqlite::params![from, to], |row| {
                Ok(MetricExportRecord {
                    timestamp: row.get(0)?,
                    cpu_usage: row.get(1)?,
                    memory_used: row.get(2)?,
                    memory_total: row.get(3)?,
                    disk_used: row.get(4)?,
                    disk_total: row.get(5)?,
                    network_upload: row.get(6)?,
                    network_download: row.get(7)?,
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

    pub fn list_frame_exports(
        &self,
        from: &str,
        to: &str,
    ) -> Result<Vec<FrameExportRecord>, StorageError> {
        // read — read_lock (independent of deletion_flag).
        let read = self.conn.read_lock();
        Self::list_frame_exports_inner(read.conn(), from, to)
    }

    /// Async `list_frame_exports` over the read funnel (ADR-026 PR-7).
    pub(crate) async fn list_frame_exports_async(
        &self,
        from: &str,
        to: &str,
    ) -> Result<Vec<FrameExportRecord>, StorageError> {
        // owned move into the Send + 'static closure (no borrowed &str).
        let from = from.to_owned();
        let to = to.to_owned();
        self.with_conn_read(move |conn| Self::list_frame_exports_inner(conn, &from, &to))
            .await
    }

    fn list_frame_exports_inner(
        conn: &rusqlite::Connection,
        from: &str,
        to: &str,
    ) -> Result<Vec<FrameExportRecord>, StorageError> {
        let mut stmt = conn
            .prepare(
                "SELECT id, timestamp, trigger_type, app_name, window_title, importance,
                        resolution_w, resolution_h, ocr_text
                 FROM frames
                 WHERE timestamp >= ?1 AND timestamp <= ?2
                 ORDER BY timestamp ASC",
            )
            .map_err(|e| StorageError::Internal(format!("Failed to prepare query: {e}")))?;

        let rows = stmt
            .query_map(rusqlite::params![from, to], |row| {
                Ok(FrameExportRecord {
                    id: row.get(0)?,
                    timestamp: row.get(1)?,
                    trigger_type: row.get(2)?,
                    app_name: row.get(3)?,
                    window_title: row.get(4)?,
                    importance: row.get(5)?,
                    resolution_w: row.get(6)?,
                    resolution_h: row.get(7)?,
                    ocr_text: row.get(8)?,
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

    pub fn count_search_frames(
        &self,
        count_sql: &str,
        pattern: Option<&str>,
    ) -> Result<u64, StorageError> {
        // read — read_lock (independent of deletion_flag).
        let read = self.conn.read_lock();
        Self::count_search_frames_inner(read.conn(), count_sql, pattern)
    }

    /// Async `count_search_frames` over the read funnel (ADR-026 PR-4).
    pub(crate) async fn count_search_frames_async(
        &self,
        count_sql: &str,
        pattern: Option<&str>,
    ) -> Result<u64, StorageError> {
        // owned move into the Send + 'static closure (no borrowed &str).
        let count_sql = count_sql.to_owned();
        let pattern = pattern.map(str::to_owned);
        self.with_conn_read(move |conn| {
            Self::count_search_frames_inner(conn, &count_sql, pattern.as_deref())
        })
        .await
    }

    fn count_search_frames_inner(
        conn: &rusqlite::Connection,
        count_sql: &str,
        pattern: Option<&str>,
    ) -> Result<u64, StorageError> {
        let count: i64 = match pattern {
            Some(p) => conn
                .query_row(count_sql, rusqlite::params![p], |row| row.get(0))
                .map_err(|e| {
                    StorageError::Internal(format!("Failed to count frame search results: {e}"))
                })?,
            None => conn
                .query_row(count_sql, [], |row| row.get(0))
                .map_err(|e| {
                    StorageError::Internal(format!("Failed to count frame search results: {e}"))
                })?,
        };

        Ok(count as u64)
    }

    pub fn search_frames_with_sql(
        &self,
        select_sql: &str,
        pattern: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<SearchFrameRow>, StorageError> {
        // read — read_lock (independent of deletion_flag).
        let read = self.conn.read_lock();
        Self::search_frames_with_sql_inner(read.conn(), select_sql, pattern, limit, offset)
    }

    /// Async `search_frames_with_sql` over the read funnel (ADR-026 PR-4).
    pub(crate) async fn search_frames_with_sql_async(
        &self,
        select_sql: &str,
        pattern: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<SearchFrameRow>, StorageError> {
        // owned move into the Send + 'static closure (no borrowed &str).
        let select_sql = select_sql.to_owned();
        let pattern = pattern.map(str::to_owned);
        self.with_conn_read(move |conn| {
            Self::search_frames_with_sql_inner(conn, &select_sql, pattern.as_deref(), limit, offset)
        })
        .await
    }

    fn search_frames_with_sql_inner(
        conn: &rusqlite::Connection,
        select_sql: &str,
        pattern: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<SearchFrameRow>, StorageError> {
        let mut stmt = conn
            .prepare(select_sql)
            .map_err(|e| StorageError::Internal(format!("Failed to prepare query: {e}")))?;

        if let Some(p) = pattern {
            let rows = stmt
                .query_map(
                    rusqlite::params![p, limit.to_string(), offset.to_string()],
                    |row| {
                        Ok(SearchFrameRow {
                            id: row.get(0)?,
                            timestamp: row.get(1)?,
                            app_name: row.get(2)?,
                            window_title: row.get(3)?,
                            matched_text: row.get(4)?,
                            importance: row.get(5)?,
                            file_path: row.get(6)?,
                        })
                    },
                )
                .map_err(|e| StorageError::Internal(format!("Failed to query frames: {e}")))?;

            let mut records = Vec::new();
            for row in rows {
                records.push(
                    row.map_err(|e| StorageError::Internal(format!("Failed to read row: {e}")))?,
                );
            }
            Ok(records)
        } else {
            let rows = stmt
                .query_map(
                    rusqlite::params![limit.to_string(), offset.to_string()],
                    |row| {
                        Ok(SearchFrameRow {
                            id: row.get(0)?,
                            timestamp: row.get(1)?,
                            app_name: row.get(2)?,
                            window_title: row.get(3)?,
                            matched_text: row.get(4)?,
                            importance: row.get(5)?,
                            file_path: row.get(6)?,
                        })
                    },
                )
                .map_err(|e| StorageError::Internal(format!("Failed to query frames: {e}")))?;

            let mut records = Vec::new();
            for row in rows {
                records.push(
                    row.map_err(|e| StorageError::Internal(format!("Failed to read row: {e}")))?,
                );
            }
            Ok(records)
        }
    }

    pub fn count_search_events(&self, pattern: &str) -> Result<u64, StorageError> {
        // read — read_lock (independent of deletion_flag).
        let read = self.conn.read_lock();
        Self::count_search_events_inner(read.conn(), pattern)
    }

    /// Async `count_search_events` over the read funnel (ADR-026 PR-4).
    pub(crate) async fn count_search_events_async(
        &self,
        pattern: &str,
    ) -> Result<u64, StorageError> {
        // owned move into the Send + 'static closure (no borrowed &str).
        let pattern = pattern.to_owned();
        self.with_conn_read(move |conn| Self::count_search_events_inner(conn, &pattern))
            .await
    }

    fn count_search_events_inner(
        conn: &rusqlite::Connection,
        pattern: &str,
    ) -> Result<u64, StorageError> {
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM events
                 WHERE data LIKE ?1",
                rusqlite::params![pattern],
                |row| row.get(0),
            )
            .map_err(|e| {
                StorageError::Internal(format!("Failed to count event search results: {e}"))
            })?;

        Ok(count as u64)
    }

    pub fn search_events(
        &self,
        pattern: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<SearchEventRow>, StorageError> {
        // read — read_lock (independent of deletion_flag).
        let read = self.conn.read_lock();
        Self::search_events_inner(read.conn(), pattern, limit, offset)
    }

    /// Async `search_events` over the read funnel (ADR-026 PR-4).
    pub(crate) async fn search_events_async(
        &self,
        pattern: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<SearchEventRow>, StorageError> {
        // owned move into the Send + 'static closure (no borrowed &str).
        let pattern = pattern.to_owned();
        self.with_conn_read(move |conn| Self::search_events_inner(conn, &pattern, limit, offset))
            .await
    }

    fn search_events_inner(
        conn: &rusqlite::Connection,
        pattern: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<SearchEventRow>, StorageError> {
        let mut stmt = conn
            .prepare(
                "SELECT event_id, timestamp,
                        json_extract(data, '$.app_name'),
                        json_extract(data, '$.window_title'),
                        data
                 FROM events
                 WHERE data LIKE ?1
                 ORDER BY timestamp DESC
                 LIMIT ?2 OFFSET ?3",
            )
            .map_err(|e| StorageError::Internal(format!("Failed to prepare query: {e}")))?;

        let rows = stmt
            .query_map(
                rusqlite::params![pattern, limit.to_string(), offset.to_string()],
                |row| {
                    Ok(SearchEventRow {
                        event_id: row.get(0)?,
                        timestamp: row.get(1)?,
                        app_name: row.get(2)?,
                        window_title: row.get(3)?,
                        data: row.get(4)?,
                    })
                },
            )
            .map_err(|e| StorageError::Internal(format!("Failed to query events: {e}")))?;

        let mut records = Vec::new();
        for row in rows {
            records
                .push(row.map_err(|e| StorageError::Internal(format!("Failed to read row: {e}")))?);
        }
        Ok(records)
    }

    /// Async GDPR Art.20 data-portability dump over the read funnel (#8056 P2-3).
    pub(crate) async fn export_personal_data_tables_async(
        &self,
    ) -> Result<Vec<PersonalDataTableExport>, StorageError> {
        self.with_conn_read(Self::export_personal_data_tables_inner)
            .await
    }

    /// Dump every [`PERSONAL_DATA_EXPORT_TABLES`] table as raw column→value JSON
    /// rows. A table absent from the current schema is skipped (never fails the
    /// whole export). Free-text PII masking is intentionally NOT applied here —
    /// the storage dump stays policy-free and the web export handler masks the
    /// known free-text columns (mirrors how the `/backup` masking lives in the
    /// web `backup_assembler`, not in storage).
    fn export_personal_data_tables_inner(
        conn: &rusqlite::Connection,
    ) -> Result<Vec<PersonalDataTableExport>, StorageError> {
        let mut out = Vec::with_capacity(PERSONAL_DATA_EXPORT_TABLES.len());
        for &table in PERSONAL_DATA_EXPORT_TABLES {
            let exists: bool = conn
                .query_row(
                    "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name = ?1",
                    rusqlite::params![table],
                    |row| row.get(0),
                )
                .unwrap_or(false);
            if !exists {
                continue;
            }
            // `table` is a fixed code-defined allowlist entry, not user input.
            let mut stmt = conn
                .prepare(&personal_data_export_query(table))
                .map_err(|e| StorageError::Internal(format!("prepare export {table}: {e}")))?;
            let column_names: Vec<String> =
                stmt.column_names().iter().map(|s| s.to_string()).collect();
            let rows: Vec<serde_json::Value> = stmt
                .query_map([], |row| {
                    let mut object = serde_json::Map::with_capacity(column_names.len());
                    for (index, name) in column_names.iter().enumerate() {
                        object.insert(name.clone(), value_ref_to_json(row.get_ref(index)?));
                    }
                    Ok(serde_json::Value::Object(object))
                })
                .map_err(|e| StorageError::Internal(format!("query export {table}: {e}")))?
                .filter_map(Result::ok)
                .collect();
            out.push(PersonalDataTableExport {
                name: table.to_string(),
                rows,
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod personal_data_export_tests {
    use super::*;

    #[test]
    fn dump_covers_allowlist_tables_and_round_trips_a_seeded_row() {
        let storage = SqliteStorage::open_in_memory(30).expect("in-memory sqlite");
        {
            let conn = storage.connection_arc();
            let guard = conn.retained_write_lock();
            guard
                .execute(
                    "INSERT INTO focus_metrics (date, focus_score) VALUES ('2026-07-10', 0.75)",
                    [],
                )
                .expect("seed focus_metrics row");
        }

        let conn = storage.connection_arc();
        let read = conn.read_lock();
        let tables =
            SqliteStorage::export_personal_data_tables_inner(read.conn()).expect("export dump");

        // Every existing allowlist table appears exactly once (empty tables too) —
        // including the V51 work-context planes (ADR-030 §12/M4), which are empty
        // here but MUST still be enumerated so a real ledger is never silently
        // dropped from the Art.20 archive.
        for expected in [
            "activity_segments",
            "ai_sessions",
            "focus_metrics",
            "work_sessions",
            "work_context_envelopes",
            "work_context_projections",
            "work_context_conflicts",
            "work_context_cursors",
            "work_context_tombstones",
        ] {
            assert_eq!(
                tables.iter().filter(|t| t.name == expected).count(),
                1,
                "{expected} must appear once in the export"
            );
        }
        // raw blobs are NEVER in the normal JSON export (§12 — reauthenticated
        // encrypted-attachment path only).
        assert_eq!(
            tables
                .iter()
                .filter(|t| t.name == "work_context_raw_blobs")
                .count(),
            0,
            "raw blobs must not appear in the normal personal-data export"
        );

        // The seeded focus_metrics row round-trips as a column->value JSON object.
        let focus = tables
            .iter()
            .find(|t| t.name == "focus_metrics")
            .expect("focus_metrics entry");
        assert_eq!(focus.rows.len(), 1, "the one seeded row must be dumped");
        assert_eq!(focus.rows[0]["date"], "2026-07-10");
        assert_eq!(focus.rows[0]["focus_score"], 0.75);
    }

    #[test]
    fn dump_omits_retracted_memory_claims_and_their_edges() {
        let storage = SqliteStorage::open_in_memory(30).expect("in-memory sqlite");
        {
            let conn = storage.connection_arc();
            let guard = conn.retained_write_lock();
            guard
                .execute_batch(
                    "INSERT INTO memory_claims
                         (claim_id, kind, text, source, confidence, status, created_at, updated_at)
                     VALUES
                         ('clm-active', 'semantic', 'active', 'test', 0.9, 'active', 1, 1),
                         ('clm-superseded', 'episodic', 'superseded', 'test', 0.8, 'superseded', 2, 2),
                         ('clm-retracted', 'procedural', 'retracted', 'test', 0.7, 'retracted', 3, 3);

                     INSERT INTO memory_edges
                         (edge_id, src_id, dst_id, edge_type, confidence, evidence_ref, source, created_at)
                     VALUES
                         ('edg-keep-evidence', 'clm-active', 'seg-safe', 'evidence', 1.0, 'seg-safe', 'rule', 1),
                         ('edg-keep-claim', 'clm-superseded', 'clm-active', 'refines', 0.8, NULL, 'rule', 2),
                         ('edg-drop-src', 'clm-retracted', 'seg-hidden', 'evidence', 1.0, 'seg-hidden', 'rule', 3),
                         ('edg-drop-dst', 'clm-active', 'clm-retracted', 'contradicts', 0.7, NULL, 'rule', 4);",
                )
                .expect("seed memory graph rows");
        }

        let conn = storage.connection_arc();
        let read = conn.read_lock();
        let tables =
            SqliteStorage::export_personal_data_tables_inner(read.conn()).expect("export dump");

        let claims = tables
            .iter()
            .find(|table| table.name == "memory_claims")
            .expect("memory_claims entry");
        let mut claim_ids: Vec<&str> = claims
            .rows
            .iter()
            .filter_map(|row| row["claim_id"].as_str())
            .collect();
        claim_ids.sort_unstable();
        assert_eq!(claim_ids, ["clm-active", "clm-superseded"]);
        assert!(claims.rows.iter().all(|row| row["status"] != "retracted"));

        let edges = tables
            .iter()
            .find(|table| table.name == "memory_edges")
            .expect("memory_edges entry");
        let mut edge_ids: Vec<&str> = edges
            .rows
            .iter()
            .filter_map(|row| row["edge_id"].as_str())
            .collect();
        edge_ids.sort_unstable();
        assert_eq!(edge_ids, ["edg-keep-claim", "edg-keep-evidence"]);
    }
}
