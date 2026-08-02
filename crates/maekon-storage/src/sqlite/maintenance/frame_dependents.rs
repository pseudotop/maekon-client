//! #9721: deleting the frame dependents SQLite's FK engine cannot reach.
//!
//! Three tables point at `frames`, but only two of them are wired to the
//! engine:
//!
//! | table | FK | who handles it |
//! |---|---|---|
//! | `frame_tags.frame_id` | `ON DELETE CASCADE` (v01_v08.rs:202) | SQLite |
//! | `interruptions.snapshot_frame_id` | `ON DELETE SET NULL` (v01_v08.rs:253) | SQLite |
//! | `frame_annotations.frame_id` | **no FK declared** (v30.rs:9) | *nobody* — this module |
//!
//! `PRAGMA foreign_keys` is **ON**: `libsqlite3-sys` builds the bundled
//! amalgamation with `-DSQLITE_DEFAULT_FOREIGN_KEYS=1`, and nothing in this
//! workspace turns it off. Measured on a live `SqliteStorage` connection:
//! `PRAGMA foreign_keys = 1`, `compile_options` contains `DEFAULT_FOREIGN_KEYS`.
//!
//! (Widespread comments in this crate claim the opposite — see #9735. They are
//! wrong. Do not add "manual cascade" statements on their authority; check
//! whether an FK is actually declared first, as the table above does.)
//!
//! So `frame_annotations` is the whole job: it declares no FK, so deleting a
//! frame left its annotations behind. That matters because the rows carry the
//! user's own `text` (highlights, memos) and BOTH callers are data-minimization
//! paths — retention expiry and "delete everything for this app". Personal
//! content was outliving the deletion meant to remove it.

use rusqlite::Transaction;

use crate::error::StorageError;

/// Delete the annotations of every frame in `[from, to]`.
///
/// Call this BEFORE deleting the parent rows, inside the same transaction.
pub(super) fn delete_frame_dependents_in_range(
    tx: &Transaction<'_>,
    from: &str,
    to: &str,
) -> Result<(), StorageError> {
    delete_orphaned_annotations(
        tx,
        "SELECT frames.id FROM frames WHERE frames.timestamp >= ?1 AND frames.timestamp <= ?2",
        rusqlite::params![from, to],
    )
}

/// Delete the annotations of every frame matching a raw predicate.
///
/// `predicate` is interpolated into the SQL, so it must be builder-controlled
/// (the app-deletion path assembles it from its own escaped patterns) — never
/// user input. It must also reference ONLY `frames` columns: the selector is
/// nested inside a `DELETE FROM frame_annotations`, and SQLite resolves
/// unqualified names innermost-first, so a predicate naming a column that
/// exists in the outer table but not in `frames` would silently become a
/// correlated subquery and match the wrong rows.
pub(super) fn delete_frame_dependents_matching(
    tx: &Transaction<'_>,
    predicate: &str,
) -> Result<(), StorageError> {
    delete_orphaned_annotations(
        tx,
        &format!("SELECT frames.id FROM frames WHERE {predicate}"),
        [],
    )
}

fn delete_orphaned_annotations<P: rusqlite::Params>(
    tx: &Transaction<'_>,
    frame_selector: &str,
    params: P,
) -> Result<(), StorageError> {
    tx.execute(
        &format!("DELETE FROM frame_annotations WHERE frame_id IN ({frame_selector})"),
        params,
    )
    .map_err(|e| StorageError::Internal(format!("frame_annotations cleanup failure: {e}")))?;
    Ok(())
}

/// Tables that hold a reference to `frames`, with how the reference is handled.
///
/// #9731 review: an FK-declaration-based check is the wrong control here, because
/// the table that actually leaked — `frame_annotations` — declares no FK and was
/// missed by exactly that method. What catches it is forcing an explicit
/// disposition for every table, the same shape as `export.rs`'s #9630 portability
/// guard: a new migration then fails CI instead of silently inheriting a default.
#[cfg(test)]
const FRAME_DEPENDENT_TABLES: &[&str] = &[
    // ON DELETE CASCADE — SQLite handles it (foreign_keys is ON; see #9735).
    "frame_tags",
    // ON DELETE SET NULL — SQLite handles it.
    "interruptions",
    // NO FK DECLARED — nothing handles it but this module. The reason the
    // module exists.
    "frame_annotations",
];

/// Tables whose DDL mentions "frame" but that hold NO frame reference.
///
/// Listed explicitly so the guard below stays a decision rather than a
/// name-matching heuristic someone loosens the first time it fires.
#[cfg(test)]
const FRAME_NAME_COLLISIONS: &[&str] = &[
    // `total_frames` is a per-session counter, not a row reference.
    "session_stats",
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlite::SqliteStorage;

    /// Every live user table must be a recorded decision: either it references
    /// `frames` (and is listed above) or it demonstrably does not.
    ///
    /// Deliberately schema-driven rather than FK-driven — see the note on
    /// `FRAME_DEPENDENT_TABLES`. A new table carrying a frame id under any
    /// column name, with or without an FK, fails this until someone classifies it.
    #[test]
    fn every_live_table_is_classified_against_the_frame_axis() {
        let storage = SqliteStorage::open_in_memory(30).expect("in-memory sqlite");
        let conn = storage.connection_arc();
        let read = conn.read_lock();

        // Same structural shadow-table filter as the #9630 export guard: fts5
        // shadows carry NULL sql or an exact known suffix off their base.
        let rows: Vec<(String, Option<String>)> = read
            .conn()
            .prepare(
                "SELECT name, sql FROM sqlite_master WHERE type = 'table' \
                 AND name NOT LIKE 'sqlite_%' ORDER BY name",
            )
            .expect("prepare sqlite_master scan")
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
            })
            .expect("scan tables")
            .filter_map(Result::ok)
            .collect();
        let fts_bases: std::collections::BTreeSet<String> = read
            .conn()
            .prepare("SELECT name FROM sqlite_master WHERE sql LIKE '%USING fts5%'")
            .expect("prepare fts scan")
            .query_map([], |row| row.get::<_, String>(0))
            .expect("scan fts")
            .filter_map(Result::ok)
            .collect();
        const FTS_SHADOW_SUFFIXES: [&str; 5] = ["_data", "_idx", "_content", "_docsize", "_config"];

        let mut unclassified = Vec::new();
        for (name, sql) in rows {
            let is_shadow = sql.is_none()
                || fts_bases.iter().any(|base| {
                    FTS_SHADOW_SUFFIXES
                        .iter()
                        .any(|suffix| name == format!("{base}{suffix}"))
                });
            if is_shadow || name == "schema_version" || name == "frames" {
                continue;
            }
            let ddl = sql.unwrap_or_default();
            if ddl.contains("frame")
                && !FRAME_DEPENDENT_TABLES.contains(&name.as_str())
                && !FRAME_NAME_COLLISIONS.contains(&name.as_str())
            {
                unclassified.push(name);
            }
        }

        assert!(
            unclassified.is_empty(),
            "table(s) mention `frame` but are not classified against the frame axis: {unclassified:?}\n\
             If the column holds a frame id, add the table to FRAME_DEPENDENT_TABLES and handle its \
             deletion (check whether an FK already does it — see #9735). If it does not (e.g. a \
             `total_frames` counter), add it to FRAME_NAME_COLLISIONS with a comment saying what the \
             column actually is."
        );
    }

    /// Both lists must name live tables — a rename would otherwise leave a
    /// stale entry silently guarding nothing, or exempting nothing.
    #[test]
    fn frame_axis_lists_name_live_tables() {
        let storage = SqliteStorage::open_in_memory(30).expect("in-memory sqlite");
        let conn = storage.connection_arc();
        let read = conn.read_lock();
        for table in FRAME_DEPENDENT_TABLES.iter().chain(FRAME_NAME_COLLISIONS) {
            let present: bool = read
                .conn()
                .query_row(
                    "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name = ?1",
                    rusqlite::params![table],
                    |row| row.get(0),
                )
                .expect("lookup table");
            assert!(
                present,
                "a frame-axis list names a table that does not exist: {table}"
            );
        }
    }
}
