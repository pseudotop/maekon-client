//! Migration V36: `egress_ledger` table — egress 감사 원장.
//!
//! 디바이스를 떠난(또는 정책상 차단된) 이벤트를 규제 준수 증거로 기록한다(#4803, E20).
//! 업로드 성공 경로와 차단(클립보드/파일접근/제외앱) 경로 모두를 disposition 으로
//! 구분하여 남긴다. `record_id` 는 호출자가 생성하는 UUID 이며 UNIQUE 제약으로
//! 재실행 시 중복을 제거한다(INSERT OR IGNORE).
//!
//! 인덱스: `occurred_at`(시계열 조회), `event_type`(유형별 집계),
//! `disposition`(업로드/차단 필터)에 각각 건다.

use rusqlite::Connection;

pub(super) fn migrate_v36(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS egress_ledger (
             id            INTEGER PRIMARY KEY AUTOINCREMENT,
             record_id     TEXT NOT NULL UNIQUE,
             event_type    TEXT NOT NULL,
             event_id      TEXT,
             byte_count    INTEGER NOT NULL,
             destination   TEXT NOT NULL,
             disposition   TEXT NOT NULL,
             consent_state TEXT NOT NULL,
             occurred_at   TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS idx_egress_ledger_occurred_at
             ON egress_ledger(occurred_at);
         CREATE INDEX IF NOT EXISTS idx_egress_ledger_event_type
             ON egress_ledger(event_type);
         CREATE INDEX IF NOT EXISTS idx_egress_ledger_disposition
             ON egress_ledger(disposition);
         INSERT OR IGNORE INTO schema_version (version) VALUES (36);",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    /// v35 상당의 최소 스키마(schema_version 만)를 만들어 현실적인 직전 상태에서
    /// 마이그레이션을 실행한다.
    fn setup_v35(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER PRIMARY KEY);
             INSERT INTO schema_version VALUES (35);",
        )
        .unwrap();
    }

    #[test]
    fn migrate_v36_creates_table() {
        let conn = Connection::open_in_memory().unwrap();
        setup_v35(&conn);
        migrate_v36(&conn).unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master \
                 WHERE type='table' AND name='egress_ledger'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "egress_ledger 테이블이 생성되어야 한다");

        // 인덱스 3종 존재 확인.
        for idx in [
            "idx_egress_ledger_occurred_at",
            "idx_egress_ledger_event_type",
            "idx_egress_ledger_disposition",
        ] {
            let c: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name=?1",
                    [idx],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(c, 1, "{idx} 인덱스가 존재해야 한다");
        }
    }

    #[test]
    fn migrate_v36_records_version() {
        let conn = Connection::open_in_memory().unwrap();
        setup_v35(&conn);
        migrate_v36(&conn).unwrap();

        let version: u32 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(version, 36);
    }

    #[test]
    fn migrate_v36_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        setup_v35(&conn);
        migrate_v36(&conn).unwrap();
        // 두 번째 호출은 CREATE TABLE/INDEX IF NOT EXISTS 로 인해 오류가 없어야 한다.
        migrate_v36(&conn).unwrap();
    }
}
