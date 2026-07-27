//! Cross-device resurrection prevention helper for periodic age-based retention DELETEs
//! (#8043).
//!
//! Running a plain DELETE (with no tombstone) on an aged-out row in a synced table
//! silently violates GDPR Art.5(1)(e) (storage-limitation): if a peer has a different
//! retention window (or was offline at the retention tick) and still holds the expired
//! row, it re-pushes that row on the next sync, and the local device re-accepts it via
//! normal LWW/AppendOnly merge → data that should have expired is resurrected.
//!
//! This module extends the same "tombstone-capture-then-delete" pattern
//! `delete_all_data_inner` already uses (#5174 S2): BEFORE the age DELETE runs, a
//! content-free suppression tombstone is recorded in the retained `sync_tombstones`
//! outbox for every expiring LOCAL-origin row. A later re-push of that same row is then
//! suppressed by `sync_merger::tombstone_suppresses`.
//!
//! Two deliberate differences from full-erasure capture:
//! * The scope is limited to `origin_device_id = <local>`. A device only tombstones its
//!   OWN origin data. Tombstoning a peer-origin row would wrongly propagate an erasure of
//!   the peer's still-live data — a peer-origin copy is aged out by its own origin device
//!   under its own retention.
//! * The tombstone is stamped at the **row's own HLC**, not a fresh erasure anchor.
//!   Retention is per-row age expiry, so suppression should only block a re-push of the
//!   same-or-older row version, while a genuinely newer peer write (a higher HLC) must
//!   still pass through via `tombstone_suppresses`'s P1 re-grant path.

use rusqlite::{Connection, OptionalExtension};

use crate::error::StorageError;
use crate::sync_table_descriptor::descriptor_for;

/// Reads the local device_id from `device_identity`. `None`/empty means this device has
/// no identity yet (= sync has never run once). In that case the caller skips tombstone
/// capture entirely: a row that has never been synced was never delivered to any peer, so
/// it cannot be resurrected.
///
/// (Note: the moment a local row is synced, `sync_extractor::backfill_origin_device_id`
/// backfills its `''` origin to this device_id, so "a local row a peer holds" always has
/// `origin = local_device_id`. Scoping capture to `origin_device_id = local` therefore
/// never misses a row that is actually at resurrection risk.)
pub(crate) fn local_device_id(conn: &Connection) -> Option<String> {
    conn.query_row(
        "SELECT device_id FROM device_identity WHERE id = 1",
        [],
        |r| r.get::<_, String>(0),
    )
    .optional()
    .ok()
    .flatten()
    .filter(|d| !d.is_empty())
}

/// Captures retention suppression tombstones for the LOCAL-origin rows that a synced
/// `table`'s age DELETE is about to remove.
///
/// `age_predicate` MUST be the **exact same** SQL predicate the caller's existing DELETE
/// uses (e.g. `"start_time < '..'"` or `"created_at < datetime('now','-90 days')"`) — so
/// the tombstone set matches the deleted row set exactly (avoids a comparison-format
/// drift). Scoped to `origin_device_id = ?1` (local); each tombstone is stamped at that
/// row's own HLC; a keep-higher `ON CONFLICT` never lowers an existing (e.g. erasure)
/// tombstone.
///
/// A no-op for a non-synced/unknown `table` (no cross-device resurrection risk there).
pub(crate) fn capture_local_origin_retention_tombstones(
    conn: &Connection,
    table: &str,
    age_predicate: &str,
    local_device_id: &str,
) -> Result<(), StorageError> {
    let Some(desc) = descriptor_for(table) else {
        return Ok(());
    };
    let row_id_sql = desc.tombstone_row_id_sql();
    conn.execute(
        &format!(
            "INSERT INTO sync_tombstones \
               (table_name, row_id, origin_device_id, hlc_wall_ms, hlc_counter, deleted_at) \
             SELECT '{table}', CAST({row_id_sql} AS TEXT), origin_device_id, \
                    hlc_wall_ms, hlc_counter, datetime('now') \
             FROM {table} WHERE ({age_predicate}) AND origin_device_id = ?1 \
             ON CONFLICT(table_name, row_id) DO UPDATE SET \
               origin_device_id = excluded.origin_device_id, \
               hlc_wall_ms = excluded.hlc_wall_ms, \
               hlc_counter = excluded.hlc_counter, \
               deleted_at  = excluded.deleted_at \
             WHERE excluded.hlc_wall_ms > sync_tombstones.hlc_wall_ms \
                OR (excluded.hlc_wall_ms = sync_tombstones.hlc_wall_ms \
                    AND excluded.hlc_counter > sync_tombstones.hlc_counter)"
        ),
        rusqlite::params![local_device_id],
    )
    .map_err(|e| {
        StorageError::Internal(format!("capture retention tombstones for {table}: {e}"))
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlite::SqliteStorage;

    /// An aged row with `origin_device_id = local` is captured; a peer-origin aged row is not.
    #[test]
    fn captures_only_local_origin_rows_at_their_own_hlc() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let (local, _) = storage.ensure_device_identity("Local").unwrap();
        let conn = storage.connection_arc();
        let guard = conn.test_lock();
        // A local-origin aged row + a peer-origin aged row.
        for (id, origin, w) in [
            ("seg-local", local.as_str(), 111u64),
            ("seg-peer", "remote", 222),
        ] {
            guard
                .execute(
                    "INSERT INTO activity_segments (id, start_time, end_time, duration_secs, \
                     trigger_reason, dominant_category, hlc_wall_ms, hlc_counter, origin_device_id) \
                     VALUES (?1, '2020-01-01T00:00:00+00:00', '2020-01-01T00:00:00+00:00', 1, \
                             'timer', 'Dev', ?2, 3, ?3)",
                    rusqlite::params![id, w, origin],
                )
                .unwrap();
        }
        capture_local_origin_retention_tombstones(
            &guard,
            "activity_segments",
            "start_time < '2021-01-01T00:00:00+00:00'",
            &local,
        )
        .unwrap();

        // Only the local row is tombstoned, at its own HLC (111,3).
        let (row_id, w, c, origin): (String, u64, u32, String) = guard
            .query_row(
                "SELECT row_id, hlc_wall_ms, hlc_counter, origin_device_id FROM sync_tombstones",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(row_id, "seg-local");
        assert_eq!((w, c), (111, 3), "tombstone stamped at the row's own HLC");
        assert_eq!(
            origin, local,
            "tombstone carries the local origin for peer convergence"
        );
        let count: i64 = guard
            .query_row("SELECT COUNT(*) FROM sync_tombstones", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1, "peer-origin aged row is NOT tombstoned");
    }

    /// keep-higher `ON CONFLICT`: an existing tombstone with a higher HLC is never lowered.
    #[test]
    fn keep_higher_hlc_never_lowers_existing_tombstone() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let (local, _) = storage.ensure_device_identity("Local").unwrap();
        let conn = storage.connection_arc();
        let guard = conn.test_lock();
        guard
            .execute(
                "INSERT INTO sync_tombstones \
                 (table_name, row_id, origin_device_id, hlc_wall_ms, hlc_counter, deleted_at) \
                 VALUES ('activity_segments', 'seg-x', ?1, 9000, 0, datetime('now'))",
                rusqlite::params![local],
            )
            .unwrap();
        guard
            .execute(
                "INSERT INTO activity_segments (id, start_time, end_time, duration_secs, \
                 trigger_reason, dominant_category, hlc_wall_ms, hlc_counter, origin_device_id) \
                 VALUES ('seg-x', '2020-01-01T00:00:00+00:00', '2020-01-01T00:00:00+00:00', 1, \
                         'timer', 'Dev', 100, 0, ?1)",
                rusqlite::params![local],
            )
            .unwrap();
        capture_local_origin_retention_tombstones(
            &guard,
            "activity_segments",
            "start_time < '2021-01-01T00:00:00+00:00'",
            &local,
        )
        .unwrap();
        let w: u64 = guard
            .query_row(
                "SELECT hlc_wall_ms FROM sync_tombstones WHERE row_id = 'seg-x'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            w, 9000,
            "existing higher-HLC tombstone is not lowered by retention capture"
        );
    }

    /// The composite-key table (`embedding_vectors`) captures its tombstone row_id as the
    /// cross-device-stable `segment_id || char(31) || model_id` — identical to the merge-side
    /// suppression key — so a later re-push of the aged embedding is actually suppressed.
    #[test]
    fn embedding_capture_uses_composite_row_id_key() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let (local, _) = storage.ensure_device_identity("Local").unwrap();
        let conn = storage.connection_arc();
        let guard = conn.test_lock();
        guard
            .execute(
                "INSERT INTO embedding_vectors (segment_id, content_type, original_text, vector, \
                 model_id, timestamp, hlc_wall_ms, hlc_counter, origin_device_id) \
                 VALUES ('seg-e', 'screen', 'txt', x'0102', 'm1', \
                         '2019-01-01T00:00:00+00:00', 777, 2, ?1)",
                rusqlite::params![local],
            )
            .unwrap();
        capture_local_origin_retention_tombstones(
            &guard,
            "embedding_vectors",
            "timestamp < '2021-01-01T00:00:00+00:00'",
            &local,
        )
        .unwrap();
        // row_id must be the composite "seg-e" + U+001F + "m1", at the row's own HLC (777,2).
        let expected_key = format!("seg-e{}m1", '\u{1f}');
        let (w, c): (u64, u32) = guard
            .query_row(
                "SELECT hlc_wall_ms, hlc_counter FROM sync_tombstones \
                 WHERE table_name = 'embedding_vectors' AND row_id = ?1",
                rusqlite::params![expected_key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("composite-keyed embedding tombstone must exist");
        assert_eq!(
            (w, c),
            (777, 2),
            "embedding tombstone stamped at the row's own HLC"
        );
    }
}
