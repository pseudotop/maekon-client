[English](./oneshim-storage.md) | [한국어](./oneshim-storage.ko.md)

# oneshim-storage

SQLite 기반 로컬 데이터 저장소 크레이트.

## 역할

- **오프라인 지원**: 네트워크 불가 시 로컬 저장
- **이벤트 이력**: 컨텍스트 이벤트 영구 저장
- **프레임 캐시**: 처리된 프레임 임시 저장
- **데이터 보존**: 설정된 기간/용량 기반 자동 정리

## 디렉토리 구조

```
oneshim-storage/src/
├── lib.rs                            # 크레이트 루트
├── sqlite/                           # SqliteStorage — StorageService 외 10+ 포트 구현 (ADR-003 디렉토리 모듈)
│   ├── mod.rs, metrics/, edge_intelligence/
│   ├── annotation_storage_impl.rs, coaching_storage.rs
│   ├── few_shot_storage_impl.rs, focus_storage_impl.rs
│   ├── frames.rs, fts_search_impl.rs, habit_storage.rs
│   ├── integration_query_impl.rs, override_store_impl.rs
│   ├── preset_storage_impl.rs, port_contract_tests.rs
├── migration/                        # 스키마 V1→V31 마이그레이션 (`CURRENT_VERSION: u32 = 31`)
│   ├── mod.rs                        # 오케스트레이터 (run_migrations, get_version)
│   ├── v01_v08.rs, v09_v18.rs, v19_v21.rs, v22_v23.rs, v23_v24.rs
│   ├── v25.rs, v26.rs, v27.rs, v28.rs, v29.rs, v30.rs
│   └── v31_regime_manager_state.rs   # 최신 V31
├── frame_storage.rs                  # 프레임 이미지 파일 저장 + 보존 정책 + 버퍼 풀
├── integration_state_store/, regime_manager_state_store.rs — orthogonal state stores
├── sync_extractor.rs, sync_merger.rs — cross-device sync
├── device_identity.rs, keychain.rs, file_secret_store.rs, env_secret_store.rs,
│   encryption.rs, process_env_projection.rs, file_transport.rs, lan_pin_store.rs —
│   credential/sync 어댑터 (superpowers-era)
└── maintenance.rs                    # 보존/vacuum 헬퍼
```

## 주요 컴포넌트

### SqliteStorage (sqlite.rs)

`StorageService` 포트 구현:

```rust
pub struct SqliteStorage {
    pool: Pool<SqliteConnectionManager>,
}

impl SqliteStorage {
    pub fn new(db_path: &Path) -> Result<Self, CoreError> {
        let manager = SqliteConnectionManager::file(db_path);
        let pool = Pool::builder()
            .max_size(5)
            .build(manager)?;

        let storage = Self { pool };
        storage.run_migrations()?;

        Ok(storage)
    }
}
```

### StorageService 구현

```rust
#[async_trait]
impl StorageService for SqliteStorage {
    async fn save_event(&self, event: &ContextEvent) -> Result<(), CoreError> {
        let conn = self.pool.get()?;
        conn.execute(
            "INSERT INTO events (event_id, event_type, window_title, app_name, timestamp, metadata)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                event.event_id,
                event.event_type.to_string(),
                event.window_title,
                event.app_name,
                event.timestamp.to_rfc3339(),
                serde_json::to_string(&event.metadata)?,
            ],
        )?;
        Ok(())
    }

    async fn get_events(&self, since: DateTime<Utc>) -> Result<Vec<ContextEvent>, CoreError> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT event_id, event_type, window_title, app_name, timestamp, metadata
             FROM events WHERE timestamp >= ?1 ORDER BY timestamp ASC"
        )?;

        let events = stmt.query_map([since.to_rfc3339()], |row| {
            // ContextEvent 변환
        })?;

        Ok(events.collect::<Result<Vec<_>, _>>()?)
    }

    async fn save_frame(&self, frame: &ProcessedFrame) -> Result<(), CoreError> {
        let conn = self.pool.get()?;
        conn.execute(
            "INSERT INTO frames (frame_id, data, processing_type, width, height, captured_at, metadata)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                frame.frame_id,
                frame.data,
                frame.processing_type.to_string(),
                frame.width,
                frame.height,
                frame.captured_at.to_rfc3339(),
                serde_json::to_string(&frame.metadata)?,
            ],
        )?;
        Ok(())
    }

    async fn cleanup_old_data(&self, before: DateTime<Utc>) -> Result<usize, CoreError> {
        let conn = self.pool.get()?;

        let events_deleted = conn.execute(
            "DELETE FROM events WHERE timestamp < ?1",
            [before.to_rfc3339()],
        )?;

        let frames_deleted = conn.execute(
            "DELETE FROM frames WHERE captured_at < ?1",
            [before.to_rfc3339()],
        )?;

        Ok(events_deleted + frames_deleted)
    }
}
```

## 데이터베이스 스키마

스키마는 V1(아래 원래 Phase-2 베이스라인)에서 **V31**(현재 — `migration/mod.rs`의 `CURRENT_VERSION: u32 = 31`)까지 진화했습니다. 각 버전별 증분 변경(컬럼 추가, 테이블 이름 변경, 신규 테이블 — focus_metrics, activity_segments, embedding_vectors, regimes, FTS5, gui_interactions, sync, IVF index, coaching, app_meta, session_audit, ai_sessions, type_confidence, regime_manager_state)은 `migration/v*.rs` 파일 참조.

### V1 스키마 (migration/v01_v08.rs, 원래 Phase-2 베이스라인)

```sql
-- events 테이블: 컨텍스트 이벤트 저장
CREATE TABLE IF NOT EXISTS events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    event_id TEXT NOT NULL UNIQUE,
    event_type TEXT NOT NULL,
    window_title TEXT,
    app_name TEXT,
    timestamp TEXT NOT NULL,
    metadata TEXT,
    created_at TEXT DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_events_timestamp ON events(timestamp);
CREATE INDEX idx_events_event_type ON events(event_type);

-- frames 테이블: 처리된 프레임 저장
CREATE TABLE IF NOT EXISTS frames (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    frame_id TEXT NOT NULL UNIQUE,
    data BLOB NOT NULL,
    processing_type TEXT NOT NULL,
    width INTEGER NOT NULL,
    height INTEGER NOT NULL,
    captured_at TEXT NOT NULL,
    metadata TEXT,
    created_at TEXT DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_frames_captured_at ON frames(captured_at);
```

### WAL 모드

```rust
fn configure_connection(conn: &Connection) -> Result<(), CoreError> {
    conn.execute_batch("
        PRAGMA journal_mode = WAL;
        PRAGMA synchronous = NORMAL;
        PRAGMA temp_store = MEMORY;
        PRAGMA mmap_size = 268435456;
    ")?;
    Ok(())
}
```

**WAL 장점**:
- 읽기/쓰기 동시 수행
- 쓰기 성능 향상
- 크래시 복구 안정성

## 데이터 보존 정책

설정 기반 자동 정리:

```rust
pub struct RetentionPolicy {
    pub max_days: u32,       // 기본 30일
    pub max_size_mb: u32,    // 기본 500MB
}

impl SqliteStorage {
    pub async fn enforce_retention(&self, policy: &RetentionPolicy) -> Result<(), CoreError> {
        // 기간 기반 정리
        let cutoff = Utc::now() - Duration::days(policy.max_days as i64);
        self.cleanup_old_data(cutoff).await?;

        // 용량 기반 정리 (필요 시)
        let size = self.get_database_size()?;
        if size > policy.max_size_mb * 1024 * 1024 {
            self.vacuum().await?;
        }

        Ok(())
    }
}
```

## 데이터베이스 위치

| 플랫폼 | 경로 |
|--------|------|
| macOS | `~/Library/Application Support/oneshim/data.db` |
| Windows | `%APPDATA%\oneshim\data.db` |
| Linux | `~/.local/share/oneshim/data.db` |

## 오프라인 동기화

네트워크 복구 시 미전송 데이터 동기화:

```rust
impl SqliteStorage {
    /// 서버에 전송되지 않은 이벤트 조회
    pub async fn get_pending_events(&self) -> Result<Vec<ContextEvent>, CoreError> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT * FROM events WHERE synced_at IS NULL ORDER BY timestamp ASC LIMIT 100"
        )?;
        // ...
    }

    /// 전송 완료 마킹
    pub async fn mark_synced(&self, event_ids: &[String]) -> Result<(), CoreError> {
        let conn = self.pool.get()?;
        let sql = format!(
            "UPDATE events SET synced_at = ?1 WHERE event_id IN ({})",
            event_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",")
        );
        // ...
    }
}
```

## 의존성

- `rusqlite`: SQLite 바인딩 (bundled 모드)
- `r2d2`: 커넥션 풀
- `directories`: 플랫폼별 데이터 디렉토리

## 테스트

```rust
#[tokio::test]
async fn test_event_crud() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let storage = SqliteStorage::new(&db_path).unwrap();

    // 이벤트 저장
    let event = ContextEvent {
        event_id: "evt_001".to_string(),
        event_type: EventType::WindowFocus,
        window_title: Some("Test Window".to_string()),
        app_name: Some("Test App".to_string()),
        timestamp: Utc::now(),
        metadata: serde_json::json!({}),
    };
    storage.save_event(&event).await.unwrap();

    // 이벤트 조회
    let since = Utc::now() - Duration::hours(1);
    let events = storage.get_events(since).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_id, "evt_001");
}

#[tokio::test]
async fn test_cleanup() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let storage = SqliteStorage::new(&db_path).unwrap();

    // 오래된 이벤트 저장
    let old_event = ContextEvent {
        timestamp: Utc::now() - Duration::days(60),
        // ...
    };
    storage.save_event(&old_event).await.unwrap();

    // 정리 실행
    let cutoff = Utc::now() - Duration::days(30);
    let deleted = storage.cleanup_old_data(cutoff).await.unwrap();
    assert_eq!(deleted, 1);
}
```

## 성능 특성

| 작업 | 예상 시간 |
|------|----------|
| 단일 이벤트 저장 | < 1ms |
| 100개 이벤트 조회 | < 5ms |
| 프레임 저장 (100KB) | < 10ms |
| VACUUM | 수초 (데이터량 의존) |
