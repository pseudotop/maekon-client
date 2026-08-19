//! Migrations V1-V8: foundation tables.
//!
//! V1: events + frames
//! V2: frames.file_path
//! V3: system_metrics + hourly aggregates
//! V4: process/idle/session + window position columns
//! V5: tags + frame_tags
//! V6: work_sessions, interruptions, focus_metrics, local_suggestions
//! V7: composite index performance optimization
//! V8: unified suggestions table

use rusqlite::Connection;
use tracing::{debug, info};

pub(super) fn migrate_v1(conn: &Connection) -> Result<(), rusqlite::Error> {
    debug!("running migration V1: events + frames tables");

    conn.execute_batch(
        "
        -- event store table
        CREATE TABLE IF NOT EXISTS events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            event_id TEXT NOT NULL UNIQUE,
            event_type TEXT NOT NULL,
            timestamp TEXT NOT NULL,
            data TEXT NOT NULL,
            is_sent INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_events_timestamp ON events(timestamp);
        CREATE INDEX IF NOT EXISTS idx_events_is_sent ON events(is_sent);
        CREATE INDEX IF NOT EXISTS idx_events_event_type ON events(event_type);

        -- frame index table
        CREATE TABLE IF NOT EXISTS frames (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            timestamp TEXT NOT NULL,
            trigger_type TEXT NOT NULL,
            app_name TEXT NOT NULL,
            window_title TEXT NOT NULL,
            importance REAL NOT NULL,
            resolution_w INTEGER NOT NULL,
            resolution_h INTEGER NOT NULL,
            has_image INTEGER NOT NULL DEFAULT 0,
            ocr_text TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_frames_timestamp ON frames(timestamp);
        CREATE INDEX IF NOT EXISTS idx_frames_app_name ON frames(app_name);

        -- version record
        INSERT INTO schema_version (version) VALUES (1);
        ",
    )?;

    info!("migration V1 completed");
    Ok(())
}

pub(super) fn migrate_v2(conn: &Connection) -> Result<(), rusqlite::Error> {
    debug!("running migration V2: add frames.file_path column");

    conn.execute_batch(
        "
        -- add file path column to the frames table
        ALTER TABLE frames ADD COLUMN file_path TEXT;

        -- file path index
        CREATE INDEX IF NOT EXISTS idx_frames_file_path ON frames(file_path);

        -- version record
        INSERT INTO schema_version (version) VALUES (2);
        ",
    )?;

    info!("migration V2 completed");
    Ok(())
}

pub(super) fn migrate_v3(conn: &Connection) -> Result<(), rusqlite::Error> {
    debug!("running migration V3: system_metrics table");

    conn.execute_batch(
        "
        -- system metrics (5-second interval)
        CREATE TABLE IF NOT EXISTS system_metrics (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            timestamp TEXT NOT NULL,
            cpu_usage REAL NOT NULL,
            memory_used INTEGER NOT NULL,
            memory_total INTEGER NOT NULL,
            disk_used INTEGER NOT NULL,
            disk_total INTEGER NOT NULL,
            network_upload INTEGER DEFAULT 0,
            network_download INTEGER DEFAULT 0,
            created_at TEXT DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_metrics_timestamp ON system_metrics(timestamp);

        -- hourly aggregates (30-day retention)
        CREATE TABLE IF NOT EXISTS system_metrics_hourly (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            hour TEXT NOT NULL UNIQUE,
            cpu_avg REAL,
            cpu_max REAL,
            memory_avg INTEGER,
            memory_max INTEGER,
            sample_count INTEGER,
            created_at TEXT DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_metrics_hourly_hour ON system_metrics_hourly(hour);

        -- version record
        INSERT INTO schema_version (version) VALUES (3);
        ",
    )?;

    info!("migration V3 completed");
    Ok(())
}

pub(super) fn migrate_v4(conn: &Connection) -> Result<(), rusqlite::Error> {
    debug!("running migration V4: process/idle/session tables");

    conn.execute_batch(
        "
        -- process snapshots (10-second interval)
        CREATE TABLE IF NOT EXISTS process_snapshots (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            timestamp TEXT NOT NULL,
            snapshot_data TEXT NOT NULL,
            created_at TEXT DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_process_timestamp ON process_snapshots(timestamp);

        -- idle periods
        CREATE TABLE IF NOT EXISTS idle_periods (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            start_time TEXT NOT NULL,
            end_time TEXT,
            duration_secs INTEGER,
            created_at TEXT DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_idle_start ON idle_periods(start_time);

        -- session statistics
        CREATE TABLE IF NOT EXISTS session_stats (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id TEXT NOT NULL UNIQUE,
            started_at TEXT NOT NULL,
            ended_at TEXT,
            total_events INTEGER DEFAULT 0,
            total_frames INTEGER DEFAULT 0,
            total_idle_secs INTEGER DEFAULT 0,
            created_at TEXT DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_session_id ON session_stats(session_id);

        -- add window position columns to the frames table
        ALTER TABLE frames ADD COLUMN window_x INTEGER;
        ALTER TABLE frames ADD COLUMN window_y INTEGER;
        ALTER TABLE frames ADD COLUMN window_width INTEGER;
        ALTER TABLE frames ADD COLUMN window_height INTEGER;

        -- version record
        INSERT INTO schema_version (version) VALUES (4);
        ",
    )?;

    info!("migration V4 completed");
    Ok(())
}

pub(super) fn migrate_v5(conn: &Connection) -> Result<(), rusqlite::Error> {
    debug!("running migration V5: tags + frame_tags tables");

    conn.execute_batch(
        "
        -- tags table
        CREATE TABLE IF NOT EXISTS tags (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL UNIQUE,
            color TEXT NOT NULL DEFAULT '#3b82f6',
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_tags_name ON tags(name);

        -- frame-tag join table
        CREATE TABLE IF NOT EXISTS frame_tags (
            frame_id INTEGER NOT NULL,
            tag_id INTEGER NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            PRIMARY KEY (frame_id, tag_id),
            FOREIGN KEY (frame_id) REFERENCES frames(id) ON DELETE CASCADE,
            FOREIGN KEY (tag_id) REFERENCES tags(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_frame_tags_frame ON frame_tags(frame_id);
        CREATE INDEX IF NOT EXISTS idx_frame_tags_tag ON frame_tags(tag_id);

        -- version record
        INSERT INTO schema_version (version) VALUES (5);
        ",
    )?;

    info!("migration V5 completed");
    Ok(())
}

pub(super) fn migrate_v6(conn: &Connection) -> Result<(), rusqlite::Error> {
    debug!("running migration V6: Edge Intelligence tables");

    conn.execute_batch(
        "
        -- work session table (tracks focus time per app category)
        CREATE TABLE IF NOT EXISTS work_sessions (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            started_at TEXT NOT NULL,
            ended_at TEXT,
            primary_app TEXT NOT NULL,
            category TEXT NOT NULL,
            state TEXT NOT NULL DEFAULT 'active',
            interruption_count INTEGER NOT NULL DEFAULT 0,
            deep_work_secs INTEGER NOT NULL DEFAULT 0,
            duration_secs INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_work_sessions_started ON work_sessions(started_at);
        CREATE INDEX IF NOT EXISTS idx_work_sessions_category ON work_sessions(category);
        CREATE INDEX IF NOT EXISTS idx_work_sessions_state ON work_sessions(state);

        -- interruptions table (tracks app-switch context)
        CREATE TABLE IF NOT EXISTS interruptions (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            interrupted_at TEXT NOT NULL,
            from_app TEXT NOT NULL,
            from_category TEXT NOT NULL,
            to_app TEXT NOT NULL,
            to_category TEXT NOT NULL,
            snapshot_frame_id INTEGER,
            resumed_at TEXT,
            resumed_to_app TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            FOREIGN KEY (snapshot_frame_id) REFERENCES frames(id) ON DELETE SET NULL
        );

        CREATE INDEX IF NOT EXISTS idx_interruptions_time ON interruptions(interrupted_at);
        CREATE INDEX IF NOT EXISTS idx_interruptions_from ON interruptions(from_app);

        -- focus metrics table (daily aggregates)
        CREATE TABLE IF NOT EXISTS focus_metrics (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            date TEXT NOT NULL UNIQUE,
            total_active_secs INTEGER NOT NULL DEFAULT 0,
            deep_work_secs INTEGER NOT NULL DEFAULT 0,
            communication_secs INTEGER NOT NULL DEFAULT 0,
            context_switches INTEGER NOT NULL DEFAULT 0,
            interruption_count INTEGER NOT NULL DEFAULT 0,
            avg_focus_duration_secs INTEGER NOT NULL DEFAULT 0,
            max_focus_duration_secs INTEGER NOT NULL DEFAULT 0,
            focus_score REAL NOT NULL DEFAULT 0.0,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE UNIQUE INDEX IF NOT EXISTS idx_focus_metrics_date ON focus_metrics(date);

        -- local suggestion table (client-only suggestions). KEPT for legacy rows +
        -- live readers (FewShotStorage, LocalSuggestionQueryPort integration source,
        -- WebStorage::list_recent_local_suggestions dashboard feed) — see the
        -- module doc on edge_intelligence/suggestions/legacy.rs (#7733).
        CREATE TABLE IF NOT EXISTS local_suggestions (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            suggestion_type TEXT NOT NULL,
            payload TEXT NOT NULL,
            shown_at TEXT,
            dismissed_at TEXT,
            acted_at TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_local_suggestions_type ON local_suggestions(suggestion_type);
        CREATE INDEX IF NOT EXISTS idx_local_suggestions_created ON local_suggestions(created_at);

        -- version record
        INSERT INTO schema_version (version) VALUES (6);
        ",
    )?;

    info!("migration V6 completed");
    Ok(())
}

pub(super) fn migrate_v7(conn: &Connection) -> Result<(), rusqlite::Error> {
    debug!("running migration V7: composite index performance optimization");

    conn.execute_batch(
        "
        -- events: optimize the unsent-event query (is_sent=0 AND timestamp ordering)
        CREATE INDEX IF NOT EXISTS idx_events_sent_timestamp ON events(is_sent, timestamp);

        -- work_sessions: optimize the active-session query (state='active' AND started_at)
        CREATE INDEX IF NOT EXISTS idx_work_sessions_state_started ON work_sessions(state, started_at);

        -- interruptions: optimize the not-yet-resumed interruption query (resumed_at IS NULL)
        CREATE INDEX IF NOT EXISTS idx_interruptions_not_resumed ON interruptions(resumed_at)
            WHERE resumed_at IS NULL;

        -- focus_metrics: optimize the date-range query
        CREATE INDEX IF NOT EXISTS idx_focus_metrics_date_score ON focus_metrics(date, focus_score);

        -- local_suggestions: optimize the pending-suggestion query
        CREATE INDEX IF NOT EXISTS idx_suggestions_pending ON local_suggestions(shown_at, acted_at, dismissed_at)
            WHERE shown_at IS NULL OR (acted_at IS NULL AND dismissed_at IS NULL);

        -- version record
        INSERT INTO schema_version (version) VALUES (7);
        ",
    )?;

    info!("migration V7 completed");
    Ok(())
}

pub(super) fn migrate_v8(conn: &Connection) -> Result<(), rusqlite::Error> {
    debug!("running migration V8: unified suggestions table");

    conn.execute_batch(
        "
        -- unified suggestions table (server + local + LLM)
        CREATE TABLE IF NOT EXISTS suggestions (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            suggestion_id TEXT NOT NULL UNIQUE,
            suggestion_type TEXT NOT NULL,
            -- Default must match SuggestionSource::RULE_BASED_STR
            source TEXT NOT NULL DEFAULT 'RULE_BASED',
            content TEXT NOT NULL,
            priority TEXT NOT NULL DEFAULT 'MEDIUM',
            confidence_score REAL NOT NULL DEFAULT 0.0,
            relevance_score REAL NOT NULL DEFAULT 0.0,
            is_actionable INTEGER NOT NULL DEFAULT 1,
            reasoning TEXT,
            shown_at TEXT,
            dismissed_at TEXT,
            acted_at TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            expires_at TEXT
        );

        CREATE INDEX IF NOT EXISTS idx_suggestions_source ON suggestions(source);
        CREATE INDEX IF NOT EXISTS idx_suggestions_created ON suggestions(created_at);
        CREATE INDEX IF NOT EXISTS idx_suggestions_type ON suggestions(suggestion_type);

        -- version record
        INSERT INTO schema_version (version) VALUES (8);
        ",
    )?;

    info!("migration V8 completed");
    Ok(())
}
