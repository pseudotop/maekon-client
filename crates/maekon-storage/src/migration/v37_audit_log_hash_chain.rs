//! Migration V37: `audit_log` 테이블에 SHA-256 해시 체인 컬럼 추가 (#4834, E20).
//!
//! ADR-072(서버 audit chain integrity policy)의 클라이언트 미러로,
//! `audit_log` 행을 tamper-**evident** 하게 만든다. 각 행에 다음을 추가한다:
//! - `seq`        — 체인 내 단조 증가 순번 (체인 편입 행에만 부여).
//! - `prev_hash`  — 직전 체인 행의 `entry_hash` (genesis 는 64-char zero hex).
//! - `entry_hash` — `SHA256(prev_hash_bytes || canonical(record))` 의 hex.
//!
//! SQLite `ALTER TABLE ADD COLUMN` 은 nullable 컬럼만 추가할 수 있으므로 세 컬럼
//! 모두 NULL 허용이다. v37 이전 작성된 legacy 행은 NULL chain 으로 남으며(체인
//! 미편입), 재해싱하지 않는다 — 작성 당시의 정렬/canonical form 을 알 수 없기
//! 때문이다. genesis 는 마이그레이션 이후 첫 write 에서 형성된다.
//!
//! `seq` 에는 partial UNIQUE INDEX(`WHERE seq IS NOT NULL`)를 걸어 legacy NULL
//! 행을 허용하면서도 체인 편입 행의 순번 유일성을 보장한다.
//!
//! ## tamper-evident vs tamper-proof
//! SHA-256-only 체인은 우발적/부분적 손상과 단순 행 편집·삭제·재정렬을 탐지한다.
//! 그러나 체인 전체를 재계산해 다시 쓸 수 있는 내부자(전면 재기록) 위협은 막지
//! 못한다 — 그것은 HMAC/Ed25519 서명(out-of-scope, 향후 `hash_version` seam)이
//! 필요하다.

use rusqlite::Connection;

/// V37: audit_log 해시 체인 컬럼(seq/prev_hash/entry_hash) + partial unique index.
pub(super) fn migrate_v37(conn: &Connection) -> Result<(), rusqlite::Error> {
    // SQLite 는 단일 ALTER 당 한 컬럼만 추가 가능 — 개별 statement 로 실행한다.
    conn.execute_batch(
        "ALTER TABLE audit_log ADD COLUMN seq INTEGER;
         ALTER TABLE audit_log ADD COLUMN prev_hash TEXT;
         ALTER TABLE audit_log ADD COLUMN entry_hash TEXT;
         CREATE UNIQUE INDEX IF NOT EXISTS idx_audit_log_seq
             ON audit_log(seq) WHERE seq IS NOT NULL;
         INSERT OR IGNORE INTO schema_version (version) VALUES (37);",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    /// v36 상당의 최소 스키마(audit_log + schema_version)를 만든다.
    fn setup_v36(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER PRIMARY KEY);
             INSERT INTO schema_version VALUES (36);
             CREATE TABLE audit_log (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 entry_id TEXT NOT NULL,
                 timestamp TEXT NOT NULL,
                 session_id TEXT NOT NULL,
                 command_id TEXT NOT NULL,
                 action_type TEXT NOT NULL,
                 status TEXT NOT NULL,
                 details TEXT,
                 execution_time_ms INTEGER,
                 UNIQUE(entry_id)
             );",
        )
        .unwrap();
    }

    fn column_names(conn: &Connection) -> Vec<String> {
        let mut stmt = conn.prepare("PRAGMA table_info(audit_log)").unwrap();
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        rows
    }

    #[test]
    fn migrate_v37_adds_chain_columns() {
        let conn = Connection::open_in_memory().unwrap();
        setup_v36(&conn);
        migrate_v37(&conn).unwrap();

        let cols = column_names(&conn);
        for c in ["seq", "prev_hash", "entry_hash"] {
            assert!(cols.iter().any(|x| x == c), "컬럼 {c} 가 추가되어야 한다");
        }
    }

    #[test]
    fn migrate_v37_creates_partial_unique_index() {
        let conn = Connection::open_in_memory().unwrap();
        setup_v36(&conn);
        migrate_v37(&conn).unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master \
                 WHERE type='index' AND name='idx_audit_log_seq'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            count, 1,
            "idx_audit_log_seq partial unique index 가 존재해야 한다"
        );
    }

    #[test]
    fn migrate_v37_records_version() {
        let conn = Connection::open_in_memory().unwrap();
        setup_v36(&conn);
        migrate_v37(&conn).unwrap();
        let version: u32 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(version, 37);
    }

    #[test]
    fn migrate_v37_legacy_rows_survive_with_null_chain() {
        let conn = Connection::open_in_memory().unwrap();
        setup_v36(&conn);
        // v37 이전에 작성된 legacy 행.
        conn.execute(
            "INSERT INTO audit_log \
             (entry_id, timestamp, session_id, command_id, action_type, status) \
             VALUES ('legacy-1', '2026-01-01T00:00:00Z', 's', 'c', 'a', 'Completed')",
            [],
        )
        .unwrap();

        migrate_v37(&conn).unwrap();

        // legacy 행이 살아있고 chain 컬럼이 NULL 이어야 한다.
        let (seq, prev, hash): (Option<i64>, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT seq, prev_hash, entry_hash FROM audit_log WHERE entry_id = 'legacy-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(seq, None);
        assert_eq!(prev, None);
        assert_eq!(hash, None);
    }

    #[test]
    fn migrate_v37_idempotent_index() {
        // 컬럼 ADD 는 멱등이 아니므로 전체 migrate 의 재실행은 보장하지 않지만,
        // partial unique index 의 IF NOT EXISTS 멱등성은 확인한다.
        let conn = Connection::open_in_memory().unwrap();
        setup_v36(&conn);
        migrate_v37(&conn).unwrap();
        conn.execute_batch(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_audit_log_seq \
             ON audit_log(seq) WHERE seq IS NOT NULL;",
        )
        .unwrap();
    }

    #[test]
    fn migrate_v37_seq_unique_allows_multiple_nulls() {
        let conn = Connection::open_in_memory().unwrap();
        setup_v36(&conn);
        migrate_v37(&conn).unwrap();
        // 여러 NULL seq 행은 허용(partial index)되어야 한다.
        conn.execute(
            "INSERT INTO audit_log (entry_id, timestamp, session_id, command_id, action_type, status) \
             VALUES ('l-1', 't', 's', 'c', 'a', 'Completed')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO audit_log (entry_id, timestamp, session_id, command_id, action_type, status) \
             VALUES ('l-2', 't', 's', 'c', 'a', 'Completed')",
            [],
        )
        .unwrap();
        // 동일한 non-NULL seq 두 개는 거부되어야 한다.
        conn.execute("UPDATE audit_log SET seq = 1 WHERE entry_id = 'l-1'", [])
            .unwrap();
        let dup_err = conn
            .execute("UPDATE audit_log SET seq = 1 WHERE entry_id = 'l-2'", [])
            .unwrap_err();
        let dup_msg = dup_err.to_string().to_uppercase();
        assert!(
            dup_msg.contains("UNIQUE"),
            "중복 seq 는 partial unique index 로 거부되어야 한다; got: {dup_err}"
        );
    }
}
