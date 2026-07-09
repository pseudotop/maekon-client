//! `HlcClock` — persistent monotonic HLC clock for stamping local synced-table writes.
//!
//! Foundation for cross-device sync of locally-authored data (F0/#5186, prereq for the
//! GDPR erasure epic #5174). Today local rows are written with `hlc_wall_ms=0` so the
//! extractor emits them at most once; this clock lets every local write carry a strictly
//! increasing HLC so ongoing changes propagate.
//!
//! ## Locking contract (critical)
//! [`HlcClock::next`] does its read-modify-write on the **already-held** `&Connection`
//! (or `&Transaction`, which derefs to `&Connection`) — it MUST be called from inside the
//! row write's existing `write_lock()`/transaction closure. It does **not** acquire its own
//! lock: the single `parking_lot::Mutex<Connection>` (`GuardedConnection`) is non-reentrant,
//! so a nested `write_lock` would deadlock. Pass `next` the **same handle the row write uses**
//! (the `tx` where a transaction is open, the `conn` otherwise) so the clock advance commits
//! or rolls back atomically with the row.
//!
//! ## Erase semantics
//! The durable floor lives in the erasable `hlc_clock` table (V39), wiped by
//! `delete_all_data_inner`. Restart re-seeds via [`seed_from_db`] from the retained
//! `app_meta["sync.erasure_hlc"]` anchor; within-session post-erase, [`HlcClock::next`]
//! self-heals (UPSERTs the singleton). Because `next` rides the row write's `write_lock`
//! (not retained), a tick is correctly skipped during erase (#4928 barrier).

use std::sync::OnceLock;

use maekon_core::sync::Hlc;
use rusqlite::{params, Connection, OptionalExtension};

/// The 6 authoritative synced tables. Used only to compute the seed floor —
/// single source of truth is `table_descriptor::ALL_TABLE_NAMES` (#7742;
/// previously a 4th hand-copied array alongside `sync_merger` and
/// `sync_extractor`'s own copies).
use crate::sync_table_descriptor::ALL_TABLE_NAMES as SYNCED_TABLES;

/// The retained `app_meta` key that anchors the clock across a GDPR wipe (written by the
/// erasure epic's S2; absent until then).
pub(crate) const ERASURE_HLC_META_KEY: &str = "sync.erasure_hlc";

/// Persistent monotonic HLC clock. Holds no counter state (the durable floor is the
/// `hlc_clock` row); memoizes the device_id to avoid a per-write `device_identity` read.
#[derive(Debug, Default)]
pub struct HlcClock {
    device_id: OnceLock<String>,
}

impl HlcClock {
    pub fn new() -> Self {
        Self::default()
    }

    /// Advance the durable clock by one tick and return the new HLC, operating on the
    /// already-held `conn` (see the locking contract in the module docs). Reuses the
    /// canonical [`Hlc::tick`] monotonicity logic over the durable floor.
    pub fn next(&self, conn: &Connection) -> rusqlite::Result<Hlc> {
        let device_id = self.device_id(conn)?;
        let (wall, counter) = read_floor(conn)?;
        // Inline of `Hlc::tick` over the durable floor, with an explicit u32 counter
        // saturation re-anchor: the canonical `tick` does `counter += 1`, which would
        // panic (debug) / silently wrap to 0 (release) at `u32::MAX` and break strict
        // monotonicity. That requires ~4e9 writes within a single millisecond (non-
        // physical), but we keep it strictly monotonic per the F0 spec by advancing the
        // wall instead of wrapping.
        let now = Hlc::now(&device_id).wall_ms;
        let next = if now > wall {
            Hlc {
                wall_ms: now,
                counter: 0,
                device_id,
            }
        } else if counter == u32::MAX {
            Hlc {
                wall_ms: wall.saturating_add(1),
                counter: 0,
                device_id,
            }
        } else {
            Hlc {
                wall_ms: wall,
                counter: counter + 1,
                device_id,
            }
        };
        write_floor(conn, next.wall_ms, next.counter)?;
        Ok(next)
    }

    /// Memoized device_id (read once from `device_identity`, lazily on first stamp).
    fn device_id(&self, conn: &Connection) -> rusqlite::Result<String> {
        if let Some(d) = self.device_id.get() {
            return Ok(d.clone());
        }
        let d: String = conn
            .query_row(
                "SELECT device_id FROM device_identity WHERE id = 1",
                [],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or_default();
        // Only memoize a REAL id. If `device_identity` isn't populated yet, return "" and
        // do NOT cache, so a later call retries (avoids permanently pinning an empty id).
        // The race-loser on `set` is harmless — the id is immutable per install.
        if !d.is_empty() {
            let _ = self.device_id.set(d.clone());
        }
        Ok(d)
    }
}

/// Read the durable floor `(wall_ms, counter)`; `(0,0)` if the singleton is absent
/// (e.g. immediately after an in-session erase wiped `hlc_clock`).
fn read_floor(conn: &Connection) -> rusqlite::Result<(u64, u32)> {
    let row = conn
        .query_row(
            "SELECT last_wall_ms, last_counter FROM hlc_clock WHERE id = 0",
            [],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
        )
        .optional()?;
    Ok(row
        .map(|(w, c)| (w.max(0) as u64, c.clamp(0, u32::MAX as i64) as u32))
        .unwrap_or((0, 0)))
}

/// Persist the durable floor (UPSERT the singleton — self-heals a wiped table).
fn write_floor(conn: &Connection, wall: u64, counter: u32) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO hlc_clock (id, last_wall_ms, last_counter) VALUES (0, ?1, ?2) \
         ON CONFLICT(id) DO UPDATE SET \
           last_wall_ms = excluded.last_wall_ms, last_counter = excluded.last_counter",
        params![wall as i64, counter as i64],
    )?;
    Ok(())
}

/// Raise the durable floor to at least `(floor_wall, floor_counter)` — never lowers it.
pub(crate) fn seed_to_floor(
    conn: &Connection,
    floor_wall: u64,
    floor_counter: u32,
) -> rusqlite::Result<()> {
    let (cur_wall, cur_counter) = read_floor(conn)?;
    if (floor_wall, floor_counter) > (cur_wall, cur_counter) {
        write_floor(conn, floor_wall, floor_counter)?;
    }
    Ok(())
}

/// Seed the clock at startup so every future `next()` dominates all existing rows.
///
/// floor = max( per-table max HLC over the 6 synced tables, retained `erasure_hlc`,
/// existing clock ). MUST run in `post_migration_setup` — i.e. before the `SqliteStorage`
/// handle is returned and any write can call `next()` — so a write can never read an
/// un-seeded clock and stamp below a retained tombstone (the erasure epic's P1 premise).
pub(crate) fn seed_from_db(conn: &Connection) -> rusqlite::Result<()> {
    let mut fw = 0u64;
    let mut fc = 0u32;

    // Per-table max HLC (mirror of compute_max_hlc's per-table MAX).
    for table in SYNCED_TABLES {
        let (w, c): (i64, i64) = conn.query_row(
            &format!(
                "SELECT COALESCE(MAX(hlc_wall_ms), 0), COALESCE(MAX(hlc_counter), 0) \
                 FROM {table} WHERE hlc_wall_ms = (\
                   SELECT COALESCE(MAX(hlc_wall_ms), 0) FROM {table}\
                 )"
            ),
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let (w, c) = (w.max(0) as u64, c.clamp(0, u32::MAX as i64) as u32);
        if (w, c) > (fw, fc) {
            (fw, fc) = (w, c);
        }
    }

    // Retained erasure anchor (written by S2; absent in F0). Survives the wipe, so it is
    // the only floor source after an erase clears the 6 tables + hlc_clock.
    if let Some(value) = conn
        .query_row(
            "SELECT value FROM app_meta WHERE key = ?1",
            params![ERASURE_HLC_META_KEY],
            |r| r.get::<_, String>(0),
        )
        .optional()?
    {
        if let Ok(e) = serde_json::from_str::<Hlc>(&value) {
            if (e.wall_ms, e.counter) > (fw, fc) {
                (fw, fc) = (e.wall_ms, e.counter);
            }
        }
    }

    seed_to_floor(conn, fw, fc)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal schema: hlc_clock singleton + device_identity + app_meta + all 6 synced
    /// tables (seed_from_db scans every one — in production they always exist post-migration).
    fn setup(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE hlc_clock (id INTEGER PRIMARY KEY CHECK (id=0), \
               last_wall_ms INTEGER NOT NULL DEFAULT 0, last_counter INTEGER NOT NULL DEFAULT 0);
             INSERT INTO hlc_clock (id,last_wall_ms,last_counter) VALUES (0,0,0);
             CREATE TABLE device_identity (id INTEGER PRIMARY KEY, device_id TEXT, device_name TEXT);
             INSERT INTO device_identity (id, device_id, device_name) VALUES (1,'dev-a','A');
             CREATE TABLE app_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE activity_segments (id TEXT PRIMARY KEY, hlc_wall_ms INTEGER NOT NULL DEFAULT 0, \
               hlc_counter INTEGER NOT NULL DEFAULT 0, origin_device_id TEXT NOT NULL DEFAULT '');
             CREATE TABLE regimes (hlc_wall_ms INTEGER NOT NULL DEFAULT 0, hlc_counter INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE regime_overrides (hlc_wall_ms INTEGER NOT NULL DEFAULT 0, hlc_counter INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE embedding_vectors (hlc_wall_ms INTEGER NOT NULL DEFAULT 0, hlc_counter INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE suggestions (hlc_wall_ms INTEGER NOT NULL DEFAULT 0, hlc_counter INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE trigger_params_snapshots (hlc_wall_ms INTEGER NOT NULL DEFAULT 0, hlc_counter INTEGER NOT NULL DEFAULT 0);",
        )
        .unwrap();
    }

    fn floor(conn: &Connection) -> (u64, u32) {
        read_floor(conn).unwrap()
    }

    #[test]
    fn next_is_strictly_monotonic_and_stamps_device_id() {
        let conn = Connection::open_in_memory().unwrap();
        setup(&conn);
        let clock = HlcClock::new();

        let a = clock.next(&conn).unwrap();
        let b = clock.next(&conn).unwrap();
        let c = clock.next(&conn).unwrap();

        assert!(
            b > a && c > b,
            "consecutive next() calls must be strictly increasing: {a:?} {b:?} {c:?}"
        );
        assert_eq!(a.device_id, "dev-a", "must stamp the device_id");
        // durable floor matches the last value.
        assert_eq!(floor(&conn), (c.wall_ms, c.counter));
    }

    #[test]
    fn next_dominates_seeded_floor_even_if_wall_unchanged() {
        let conn = Connection::open_in_memory().unwrap();
        setup(&conn);
        // Seed with a future wall → calls within the same ms advance via the counter.
        let future = u64::try_from(4_000_000_000_000i64).unwrap();
        seed_to_floor(&conn, future, 7).unwrap();
        let clock = HlcClock::new();

        let a = clock.next(&conn).unwrap();
        assert!(
            a.wall_ms > future || (a.wall_ms == future && a.counter > 7),
            "must dominate the seeded floor: {a:?} vs ({future},7)"
        );
    }

    #[test]
    fn next_self_heals_when_singleton_missing() {
        let conn = Connection::open_in_memory().unwrap();
        setup(&conn);
        conn.execute("DELETE FROM hlc_clock", []).unwrap(); // simulate an in-session erase
        let clock = HlcClock::new();
        let a = clock.next(&conn).unwrap();
        assert!(
            a.wall_ms > 0,
            "stamps from the wall-clock even with an empty table"
        );
        assert_eq!(
            floor(&conn),
            (a.wall_ms, a.counter),
            "singleton recovered via UPSERT"
        );
    }

    #[test]
    fn seed_from_db_takes_max_of_tables_and_erasure_anchor() {
        let conn = Connection::open_in_memory().unwrap();
        setup(&conn);
        // Per-table max HLC = (500, 2).
        conn.execute(
            "INSERT INTO activity_segments (id, hlc_wall_ms, hlc_counter, origin_device_id) \
             VALUES ('s1', 500, 2, 'dev-a')",
            [],
        )
        .unwrap();
        // retained erasure anchor = (900, 0) > table.
        conn.execute(
            "INSERT INTO app_meta (key, value) VALUES (?1, ?2)",
            params![
                ERASURE_HLC_META_KEY,
                serde_json::to_string(&Hlc {
                    wall_ms: 900,
                    counter: 0,
                    device_id: "dev-a".into()
                })
                .unwrap()
            ],
        )
        .unwrap();

        seed_from_db(&conn).unwrap();
        assert_eq!(
            floor(&conn),
            (900, 0),
            "floor must be the max of the table and the anchor"
        );

        // A subsequent next() dominates the anchor.
        let a = HlcClock::new().next(&conn).unwrap();
        assert!(
            a.wall_ms > 900 || (a.wall_ms == 900 && a.counter > 0),
            "{a:?}"
        );
    }

    #[test]
    fn seed_never_lowers_the_floor() {
        let conn = Connection::open_in_memory().unwrap();
        setup(&conn);
        seed_to_floor(&conn, 1000, 5).unwrap();
        seed_to_floor(&conn, 10, 0).unwrap(); // a lower value
        assert_eq!(floor(&conn), (1000, 5), "seeding never lowers the floor");
    }

    #[test]
    fn seed_picks_higher_counter_at_equal_max_wall() {
        let conn = Connection::open_in_memory().unwrap();
        setup(&conn);
        conn.execute(
            "INSERT INTO suggestions (hlc_wall_ms, hlc_counter) VALUES (700, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO suggestions (hlc_wall_ms, hlc_counter) VALUES (700, 9)",
            [],
        )
        .unwrap();
        seed_from_db(&conn).unwrap();
        assert_eq!(
            floor(&conn),
            (700, 9),
            "must pick the larger counter as the floor at an equal max wall"
        );
    }

    #[test]
    fn next_reanchors_wall_instead_of_wrapping_counter_at_saturation() {
        let conn = Connection::open_in_memory().unwrap();
        setup(&conn);
        // Seed with a future wall + counter=MAX → since now is less than wall, the tick
        // path attempts counter+1, but on saturation it must advance the wall (no wrap).
        let future = 4_000_000_000_000u64;
        seed_to_floor(&conn, future, u32::MAX).unwrap();
        let a = HlcClock::new().next(&conn).unwrap();
        assert_eq!(
            (a.wall_ms, a.counter),
            (future + 1, 0),
            "on counter saturation it must advance the wall instead of wrapping to 0: {a:?}"
        );
        assert!(
            (a.wall_ms, a.counter) > (future, u32::MAX),
            "the re-anchored result must be strictly greater than the previous floor"
        );
    }
}
