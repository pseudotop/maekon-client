//! Migration V38: `sync_tombstones` table — 교차기기 삭제 수렴용 보존 아웃박스.
//!
//! GDPR Art. 17 consent-revoke 시, 로컬에서 하드-와이프되는 동기화 행들의 *스켈레톤*
//! (id + HLC + deleted_at)만을 보존하여, 오프라인이던 피어가 나중에 접속해 일반 sync
//! 스트림으로 텀스톤을 받아 수렴(hard-delete + 부활 억제)하도록 한다. Public
//! companion: `docs/guides/sync-conflict-resolution.md`.
//!
//! **무(無) PII**: `table_name`/`row_id`/`origin_device_id`/HLC/`deleted_at` 만 담는다.
//! 본문 컬럼(llm_summary, original_text 등)은 절대 담기지 않으므로 디스크/와이어에
//! 삭제된 사용자 콘텐츠가 잔존하지 않는다. 이 보존 근거는 `egress_ledger`(V36)·
//! `audit_log` 과 동일한 GDPR Art. 17(3) 처리-기록 기반이다.
//!
//! `delete_all_data_inner` 의 `ALL_TABLES` 에 **의도적으로 포함하지 않는다**(보존). 실제
//! 텀스톤 생성(erase 시 id 캡처 → INSERT)은 S2(#5179)에서 `delete_all_data_inner`
//! 트랜잭션에 합류한다 — 이 마이그레이션은 스키마만 만든다(behavior-neutral).
//!
//! 보존 기간: 영구가 아니다. 재동의(re-grant) 후 더 높은 HLC 행이 텀스톤을 supersede 하면
//! S3 이 해당 텀스톤을 조기 제거할 수 있고(#5180), 그 외에는 텀스톤 GC(`max(retention,90)d`,
//! S5/#5182)가 상한을 강제한다 — 메타데이터 최소 보존(GDPR Art. 5(1)(e)).
//!
//! 인덱스: `(hlc_wall_ms, hlc_counter)` — extractor 가 `HLC > watermark` 로 아웃박스
//! 행을 추출하는 경로의 정렬/필터용.

use rusqlite::Connection;

pub(super) fn migrate_v38(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS sync_tombstones (
             table_name       TEXT    NOT NULL,
             row_id           TEXT    NOT NULL,
             origin_device_id TEXT    NOT NULL,
             hlc_wall_ms      INTEGER NOT NULL,
             hlc_counter      INTEGER NOT NULL,
             deleted_at       TEXT    NOT NULL,
             PRIMARY KEY (table_name, row_id)
         );
         CREATE INDEX IF NOT EXISTS idx_sync_tombstones_hlc
             ON sync_tombstones(hlc_wall_ms, hlc_counter);
         INSERT OR IGNORE INTO schema_version (version) VALUES (38);",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    /// v37 상당의 최소 스키마(schema_version 만)에서 마이그레이션을 실행한다.
    fn setup_v37(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER PRIMARY KEY);
             INSERT INTO schema_version VALUES (37);",
        )
        .unwrap();
    }

    #[test]
    fn migrate_v38_creates_table() {
        let conn = Connection::open_in_memory().unwrap();
        setup_v37(&conn);
        migrate_v38(&conn).unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master \
                 WHERE type='table' AND name='sync_tombstones'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "sync_tombstones 테이블이 생성되어야 한다");

        let idx: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master \
                 WHERE type='index' AND name='idx_sync_tombstones_hlc'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(idx, 1, "HLC 인덱스가 존재해야 한다");
    }

    #[test]
    fn migrate_v38_composite_pk_dedups() {
        let conn = Connection::open_in_memory().unwrap();
        setup_v37(&conn);
        migrate_v38(&conn).unwrap();

        // row_id 는 라이브 테이블의 합성 키(ULID/UUID/autoincrement)일 뿐 사용자 콘텐츠가
        // 아니다 — 불투명 키 성격을 드러내려 UUID 형식 문자열을 사용한다.
        let rid = "01920e1f-0000-7000-8000-000000000001";
        conn.execute(
            "INSERT INTO sync_tombstones \
             (table_name, row_id, origin_device_id, hlc_wall_ms, hlc_counter, deleted_at) \
             VALUES ('activity_segments',?1,'dev-a',100,0,'2026-06-05T00:00:00Z')",
            [rid],
        )
        .unwrap();
        // 같은 (table_name,row_id) 재삽입은 PK 충돌 — SQLite 가 복합 PK upsert 를 지원함을
        // 확인할 뿐이다(실제 producer 의 upsert API 선택은 S2/S3 에서 결정).
        conn.execute(
            "INSERT OR REPLACE INTO sync_tombstones \
             (table_name, row_id, origin_device_id, hlc_wall_ms, hlc_counter, deleted_at) \
             VALUES ('activity_segments',?1,'dev-a',200,1,'2026-06-05T01:00:00Z')",
            [rid],
        )
        .unwrap();

        let (rows, wall): (i64, i64) = conn
            .query_row(
                "SELECT COUNT(*), MAX(hlc_wall_ms) FROM sync_tombstones \
                 WHERE table_name='activity_segments' AND row_id=?1",
                [rid],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(rows, 1, "복합 PK 로 (table,row) 당 1행이어야 한다");
        assert_eq!(wall, 200, "REPLACE 로 더 높은 HLC 가 유지되어야 한다");
    }

    #[test]
    fn migrate_v38_records_version() {
        let conn = Connection::open_in_memory().unwrap();
        setup_v37(&conn);
        migrate_v38(&conn).unwrap();

        let version: u32 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(version, 38);
    }

    #[test]
    fn migrate_v38_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        setup_v37(&conn);
        migrate_v38(&conn).unwrap();
        migrate_v38(&conn).unwrap();
    }
}
