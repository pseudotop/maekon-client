//! Migration V39: `hlc_clock` singleton — 로컬 쓰기용 영속 단조 HLC 클럭.
//!
//! 교차기기 sync 가 로컬 데이터를 실제로 전파하려면 모든 로컬 synced-table 쓰기가
//! 단조 증가하는 HLC 로 스탬프되어야 한다(Public companion:
//! `docs/guides/sync-conflict-resolution.md`, F0/#5186). 이 테이블은
//! 그 클럭의 **durable floor** 를 담는 싱글톤(`id=0`)이다. 실제 스탬핑(write-site 배선)은
//! 후속 PR; 이 마이그레이션은 테이블만 만든다(behavior-neutral).
//!
//! **erase 정책:** `hlc_clock` 은 활동-타이밍 메타데이터(wall_ms)이므로 GDPR Art.17
//! 소거 대상이다 — `delete_all_data_inner` 의 `ALL_TABLES` 에 포함된다(보존 아님). 소거
//! 후에는 재시작 시 `post_migration_setup` 이 retained `app_meta["sync.erasure_hlc"]`
//! 기준으로 재시드한다. (대조: `erasure_hlc` 단일값은 retained `app_meta`.)

use rusqlite::Connection;

pub(super) fn migrate_v39(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS hlc_clock (
             id           INTEGER PRIMARY KEY CHECK (id = 0),
             last_wall_ms INTEGER NOT NULL DEFAULT 0,
             last_counter INTEGER NOT NULL DEFAULT 0
         );
         INSERT OR IGNORE INTO hlc_clock (id, last_wall_ms, last_counter) VALUES (0, 0, 0);
         INSERT OR IGNORE INTO schema_version (version) VALUES (39);",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn setup_v38(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER PRIMARY KEY);
             INSERT INTO schema_version VALUES (38);",
        )
        .unwrap();
    }

    #[test]
    fn migrate_v39_creates_singleton() {
        let conn = Connection::open_in_memory().unwrap();
        setup_v38(&conn);
        migrate_v39(&conn).unwrap();

        let (rows, wall, counter): (i64, i64, i64) = conn
            .query_row(
                "SELECT COUNT(*), COALESCE(MAX(last_wall_ms),0), COALESCE(MAX(last_counter),0) \
                 FROM hlc_clock",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(rows, 1, "싱글톤 행 1개가 시드되어야 한다");
        assert_eq!((wall, counter), (0, 0), "초기 floor 는 (0,0)");
    }

    #[test]
    fn migrate_v39_singleton_constraint() {
        let conn = Connection::open_in_memory().unwrap();
        setup_v38(&conn);
        migrate_v39(&conn).unwrap();
        // id != 0 는 CHECK 제약 위반.
        let insert_err = conn
            .execute(
                "INSERT INTO hlc_clock (id, last_wall_ms, last_counter) VALUES (1, 5, 0)",
                [],
            )
            .unwrap_err();
        let msg = insert_err.to_string().to_uppercase();
        assert!(
            msg.contains("CHECK"),
            "id=1 삽입은 CHECK(id=0) 위반이어야 한다; got: {insert_err}"
        );
    }

    #[test]
    fn migrate_v39_records_version() {
        let conn = Connection::open_in_memory().unwrap();
        setup_v38(&conn);
        migrate_v39(&conn).unwrap();
        let version: u32 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 39);
    }

    #[test]
    fn migrate_v39_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        setup_v38(&conn);
        migrate_v39(&conn).unwrap();
        migrate_v39(&conn).unwrap();
    }
}
