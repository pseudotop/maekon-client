//! Migrations V9–V18: tiered memory, vectors, sync, IVF, coaching, trigram FTS.
//!
//! V9:  calibration_log, trigger_params_snapshots, regimes, activity_segments
//! V10: embedding_vectors, weekly_digests
//! V11: FTS5 search_fts, daily_digests
//! V12: regime_overrides (recalibration)
//! V13: gui_interactions
//! V14: INT8 quantization columns + cross-device sync metadata (HLC, tombstones)
//! V15: lan_peer_pins (Sync 3b TOFU)
//! V16: IVF index + 2-bit binary codes
//! V17: coaching engine tables
//! V18: Korean trigram FTS5 table (search_trigram)
//! V19: app_meta key-value table (onboarding state, etc.)

use rusqlite::Connection;
use tracing::{debug, info, warn};

pub(super) fn migrate_v9(conn: &Connection) -> Result<(), rusqlite::Error> {
    debug!("migration V9 execution: tiered memory tables (calibration, regimes, segments)");

    conn.execute_batch(
        "
        -- Calibration log: one row per trigger event for offline tuning
        CREATE TABLE IF NOT EXISTS calibration_log (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            timestamp TEXT NOT NULL,
            event_type TEXT NOT NULL,
            app_name TEXT NOT NULL,
            app_category TEXT NOT NULL,
            event_importance REAL NOT NULL,
            density_signal REAL NOT NULL,
            importance_signal REAL NOT NULL,
            context_signal REAL NOT NULL,
            buffer_signal REAL NOT NULL,
            trigger_score REAL NOT NULL,
            trigger_action TEXT,
            active_regime_id TEXT,
            params_version_id TEXT NOT NULL,
            is_noise INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_calibration_timestamp ON calibration_log(timestamp);
        CREATE INDEX IF NOT EXISTS idx_calibration_regime ON calibration_log(active_regime_id);
        CREATE INDEX IF NOT EXISTS idx_calibration_noise ON calibration_log(is_noise);
        CREATE INDEX IF NOT EXISTS idx_calibration_ts_noise ON calibration_log(timestamp, is_noise);

        -- Trigger parameter snapshots: immutable record per version
        CREATE TABLE IF NOT EXISTS trigger_params_snapshots (
            id TEXT PRIMARY KEY,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            preset TEXT NOT NULL,
            params_json TEXT NOT NULL
        );

        -- Regimes: detected behavioral regimes (clusters of similar activity)
        CREATE TABLE IF NOT EXISTS regimes (
            id TEXT PRIMARY KEY,
            label TEXT NOT NULL,
            detected_at TEXT NOT NULL,
            last_seen_at TEXT NOT NULL,
            occurrence_count INTEGER NOT NULL DEFAULT 1,
            avg_density REAL NOT NULL DEFAULT 0.0,
            avg_importance REAL NOT NULL DEFAULT 0.0,
            dominant_category TEXT NOT NULL,
            params_snapshot_id TEXT,
            is_active INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            FOREIGN KEY (params_snapshot_id) REFERENCES trigger_params_snapshots(id)
        );

        CREATE INDEX IF NOT EXISTS idx_regimes_active ON regimes(is_active);
        CREATE INDEX IF NOT EXISTS idx_regimes_last_seen ON regimes(last_seen_at);

        -- Activity segments: closed segments produced by AdaptiveTrigger
        CREATE TABLE IF NOT EXISTS activity_segments (
            id TEXT PRIMARY KEY,
            start_time TEXT NOT NULL,
            end_time TEXT NOT NULL,
            duration_secs INTEGER NOT NULL,
            regime_id TEXT,
            trigger_reason TEXT NOT NULL,
            event_count INTEGER NOT NULL DEFAULT 0,
            app_breakdown TEXT NOT NULL DEFAULT '{}',
            category_breakdown TEXT NOT NULL DEFAULT '{}',
            context_switch_count INTEGER NOT NULL DEFAULT 0,
            dominant_category TEXT NOT NULL,
            avg_importance REAL NOT NULL DEFAULT 0.0,
            patterns_json TEXT NOT NULL DEFAULT '[]',
            content_activities_json TEXT NOT NULL DEFAULT '[]',
            container_json TEXT,
            llm_summary TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            FOREIGN KEY (regime_id) REFERENCES regimes(id)
        );

        CREATE INDEX IF NOT EXISTS idx_segments_start ON activity_segments(start_time);
        CREATE INDEX IF NOT EXISTS idx_segments_regime ON activity_segments(regime_id);
        CREATE INDEX IF NOT EXISTS idx_segments_reason ON activity_segments(trigger_reason);

        -- version record
        INSERT INTO schema_version (version) VALUES (9);
        ",
    )?;

    info!("migration V9 completed");
    Ok(())
}

pub(super) fn migrate_v10(conn: &Connection) -> Result<(), rusqlite::Error> {
    debug!("migration V10 execution: embedding vectors and weekly digests tables");

    conn.execute_batch(
        "
        -- Embedding vectors table (raw vectors stored as BLOB, indexed by sqlite-vec at runtime)
        CREATE TABLE IF NOT EXISTS embedding_vectors (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            segment_id TEXT NOT NULL,
            content_type TEXT NOT NULL,
            content_label TEXT,
            original_text TEXT NOT NULL,
            vector BLOB NOT NULL,
            model_id TEXT NOT NULL,
            timestamp TEXT NOT NULL,
            is_stale INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_embedding_segment ON embedding_vectors(segment_id);
        CREATE INDEX IF NOT EXISTS idx_embedding_timestamp ON embedding_vectors(timestamp);
        CREATE INDEX IF NOT EXISTS idx_embedding_model ON embedding_vectors(model_id);
        CREATE INDEX IF NOT EXISTS idx_embedding_stale ON embedding_vectors(is_stale);

        -- Weekly digest table (aggregated stats per week)
        CREATE TABLE IF NOT EXISTS weekly_digests (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            week_start TEXT NOT NULL,
            week_end TEXT NOT NULL,
            stats_json TEXT NOT NULL,
            comparison_json TEXT,
            llm_narrative TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE UNIQUE INDEX IF NOT EXISTS idx_digest_week ON weekly_digests(week_start);

        -- version record
        INSERT INTO schema_version (version) VALUES (10);
        ",
    )?;

    info!("migration V10 completed");
    Ok(())
}

pub(super) fn migrate_v11(conn: &Connection) -> Result<(), rusqlite::Error> {
    debug!("migration V11 execution: FTS5 search index + daily digests table");

    // FTS5 virtual table — may fail if the fts5 extension is not compiled in.
    // We log a warning and continue; the TextSearchProvider can return empty results.
    let fts5_result = conn.execute_batch(
        "
        -- FTS5 full-text search index
        CREATE VIRTUAL TABLE IF NOT EXISTS search_fts USING fts5(
            segment_id UNINDEXED,
            content_type,
            searchable_text,
            tokenize='porter unicode61'
        );

        -- Backfill existing segments
        INSERT OR IGNORE INTO search_fts (segment_id, content_type, searchable_text)
        SELECT id, 'segment', COALESCE(llm_summary, '') || ' ' || COALESCE(dominant_category, '')
        FROM activity_segments;
        ",
    );
    if let Err(e) = fts5_result {
        tracing::warn!("FTS5 table creation skipped (extension not available): {e}");
    }

    conn.execute_batch(
        "
        -- Daily digests
        CREATE TABLE IF NOT EXISTS daily_digests (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            date TEXT NOT NULL UNIQUE,
            insight_json TEXT,
            timeline_json TEXT NOT NULL,
            statistics_json TEXT NOT NULL,
            generated_at TEXT NOT NULL
        );

        -- version record
        INSERT INTO schema_version (version) VALUES (11);
        ",
    )?;

    info!("migration V11 completed");
    Ok(())
}

pub(super) fn migrate_v12(conn: &Connection) -> Result<(), rusqlite::Error> {
    debug!("migration V12 execution: regime_overrides table for recalibration");

    conn.execute_batch(
        "
        -- User regime overrides for constraint-based re-clustering
        CREATE TABLE IF NOT EXISTS regime_overrides (
            override_id TEXT PRIMARY KEY,
            segment_id TEXT NOT NULL,
            original_regime_id TEXT,
            action_type TEXT NOT NULL,
            action_data TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_override_segment ON regime_overrides(segment_id);
        CREATE INDEX IF NOT EXISTS idx_override_created ON regime_overrides(created_at);

        -- version record
        INSERT INTO schema_version (version) VALUES (12);
        ",
    )?;

    info!("migration V12 completed");
    Ok(())
}

pub(super) fn migrate_v13(conn: &Connection) -> Result<(), rusqlite::Error> {
    debug!("migration V13 execution: gui_interactions table for GUI Activity Intelligence");

    conn.execute_batch(
        "
        -- GUI interaction events for Phase 2 GUI Activity Intelligence
        CREATE TABLE IF NOT EXISTS gui_interactions (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            event_id TEXT NOT NULL,
            segment_id TEXT,
            timestamp TEXT NOT NULL,
            element_text TEXT,
            element_type TEXT,
            interaction_type TEXT NOT NULL,
            bbox_json TEXT,
            app_name TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_gui_segment ON gui_interactions(segment_id);
        CREATE INDEX IF NOT EXISTS idx_gui_timestamp ON gui_interactions(timestamp);

        -- version record
        INSERT INTO schema_version (version) VALUES (13);
        ",
    )?;

    info!("migration V13 completed");
    Ok(())
}

pub(super) fn migrate_v14(conn: &Connection) -> Result<(), rusqlite::Error> {
    debug!("migration V14 execution: INT8 quantization columns + cross-device sync metadata");

    conn.execute_batch(
        "
        -- === P3 Vector Compression: INT8 quantized vector columns ===
        ALTER TABLE embedding_vectors ADD COLUMN vector_int8 BLOB;
        ALTER TABLE embedding_vectors ADD COLUMN quant_scale REAL;
        ALTER TABLE embedding_vectors ADD COLUMN quant_offset REAL;

        -- === P3 Cross-Device Sync: HLC columns on syncable tables ===
        ALTER TABLE activity_segments ADD COLUMN hlc_wall_ms INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE activity_segments ADD COLUMN hlc_counter INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE activity_segments ADD COLUMN origin_device_id TEXT NOT NULL DEFAULT '';

        ALTER TABLE regimes ADD COLUMN hlc_wall_ms INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE regimes ADD COLUMN hlc_counter INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE regimes ADD COLUMN origin_device_id TEXT NOT NULL DEFAULT '';

        ALTER TABLE regime_overrides ADD COLUMN hlc_wall_ms INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE regime_overrides ADD COLUMN hlc_counter INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE regime_overrides ADD COLUMN origin_device_id TEXT NOT NULL DEFAULT '';

        ALTER TABLE embedding_vectors ADD COLUMN hlc_wall_ms INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE embedding_vectors ADD COLUMN hlc_counter INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE embedding_vectors ADD COLUMN origin_device_id TEXT NOT NULL DEFAULT '';

        ALTER TABLE suggestions ADD COLUMN hlc_wall_ms INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE suggestions ADD COLUMN hlc_counter INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE suggestions ADD COLUMN origin_device_id TEXT NOT NULL DEFAULT '';

        ALTER TABLE trigger_params_snapshots ADD COLUMN hlc_wall_ms INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE trigger_params_snapshots ADD COLUMN hlc_counter INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE trigger_params_snapshots ADD COLUMN origin_device_id TEXT NOT NULL DEFAULT '';

        -- Tombstone columns for LWW-managed tables only
        ALTER TABLE regimes ADD COLUMN is_deleted INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE regimes ADD COLUMN deleted_at TEXT;

        ALTER TABLE suggestions ADD COLUMN is_deleted INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE suggestions ADD COLUMN deleted_at TEXT;

        ALTER TABLE embedding_vectors ADD COLUMN is_deleted INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE embedding_vectors ADD COLUMN deleted_at TEXT;

        -- Sync infrastructure tables
        CREATE TABLE IF NOT EXISTS sync_peers (
            device_id TEXT PRIMARY KEY,
            device_name TEXT NOT NULL,
            last_sync_at TEXT NOT NULL,
            watermark_wall_ms INTEGER NOT NULL DEFAULT 0,
            watermark_counter INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS device_identity (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            device_id TEXT NOT NULL UNIQUE,
            device_name TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        -- version record
        INSERT INTO schema_version (version) VALUES (14);
        ",
    )?;

    info!("migration V14 completed");
    Ok(())
}

pub(super) fn migrate_v15(conn: &Connection) -> Result<(), rusqlite::Error> {
    debug!("migration V15 execution: LAN sync peer TOFU pins table");

    conn.execute_batch(
        "
        -- LAN peer TOFU (Trust On First Use) certificate pins
        CREATE TABLE IF NOT EXISTS lan_peer_pins (
            device_id TEXT PRIMARY KEY,
            cert_fingerprint TEXT NOT NULL,
            first_seen_at TEXT NOT NULL DEFAULT (datetime('now')),
            last_seen_at TEXT NOT NULL DEFAULT (datetime('now')),
            trust_revoked INTEGER NOT NULL DEFAULT 0
        );

        -- version record
        INSERT INTO schema_version (version) VALUES (15);
        ",
    )?;

    info!("migration V15 completed");
    Ok(())
}

pub(super) fn migrate_v16(conn: &Connection) -> Result<(), rusqlite::Error> {
    debug!("migration V16 execution: IVF index + 2-bit binary codes for vector search");

    conn.execute_batch(
        "
        -- 2-bit binary codes for Hamming distance filtering
        CREATE TABLE IF NOT EXISTS vector_binary_codes (
            vector_id INTEGER PRIMARY KEY,
            binary_code BLOB NOT NULL,
            FOREIGN KEY (vector_id) REFERENCES embedding_vectors(id) ON DELETE CASCADE
        );

        -- IVF cluster centroids (INT8 format)
        CREATE TABLE IF NOT EXISTS ivf_centroids (
            id INTEGER PRIMARY KEY,
            centroid_int8 BLOB NOT NULL,
            centroid_scale REAL NOT NULL,
            centroid_offset REAL NOT NULL,
            vector_count INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        -- IVF cluster memberships
        CREATE TABLE IF NOT EXISTS ivf_assignments (
            vector_id INTEGER PRIMARY KEY,
            cluster_id INTEGER NOT NULL,
            FOREIGN KEY (vector_id) REFERENCES embedding_vectors(id) ON DELETE CASCADE,
            FOREIGN KEY (cluster_id) REFERENCES ivf_centroids(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_ivf_assign_cluster ON ivf_assignments(cluster_id);

        -- Index build metadata (key-value store)
        CREATE TABLE IF NOT EXISTS vector_index_meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL,
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        -- version record
        INSERT INTO schema_version (version) VALUES (16);
        ",
    )?;

    info!("migration V16 completed");
    Ok(())
}

pub(super) fn migrate_v17(conn: &Connection) -> Result<(), rusqlite::Error> {
    debug!("migration V17 execution: coaching engine tables");

    conn.execute_batch(
        "
        -- Coaching event log: every coaching message shown
        CREATE TABLE IF NOT EXISTS coaching_events (
            id                    INTEGER PRIMARY KEY AUTOINCREMENT,
            event_id              TEXT NOT NULL UNIQUE,
            trigger_type          TEXT NOT NULL,
            profile_name          TEXT NOT NULL,
            regime_id             TEXT,
            message_template      TEXT NOT NULL,
            personalized_message  TEXT,
            shown_at              TEXT NOT NULL,
            dismissed_at          TEXT,
            dismiss_action        TEXT,
            feedback_type         TEXT,
            feedback_score        REAL,
            behavior_change_detected INTEGER DEFAULT 0,
            created_at            TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_coaching_events_profile
            ON coaching_events(profile_name, shown_at);
        CREATE INDEX IF NOT EXISTS idx_coaching_events_regime
            ON coaching_events(regime_id, shown_at);

        -- Per-regime daily time goals (user-configured)
        CREATE TABLE IF NOT EXISTS regime_goals (
            id                 INTEGER PRIMARY KEY AUTOINCREMENT,
            regime_label       TEXT NOT NULL UNIQUE,
            daily_target_minutes INTEGER NOT NULL,
            created_at         TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at         TEXT NOT NULL DEFAULT (datetime('now'))
        );

        -- Aggregated coaching effectiveness scores
        CREATE TABLE IF NOT EXISTS coaching_effectiveness (
            id                  INTEGER PRIMARY KEY AUTOINCREMENT,
            profile_name        TEXT NOT NULL,
            trigger_type        TEXT NOT NULL,
            total_shown         INTEGER NOT NULL DEFAULT 0,
            positive_feedback   REAL NOT NULL DEFAULT 0.0,
            negative_feedback   REAL NOT NULL DEFAULT 0.0,
            neutral_count       INTEGER NOT NULL DEFAULT 0,
            behavior_change_count INTEGER NOT NULL DEFAULT 0,
            updated_at          TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(profile_name, trigger_type)
        );

        INSERT INTO schema_version (version) VALUES (17);
        ",
    )?;

    info!("migration V17 complete: coaching engine tables created");
    Ok(())
}

pub(super) fn migrate_v18(conn: &Connection) -> Result<(), rusqlite::Error> {
    debug!("migration V18 execution: Korean trigram FTS5 table (search_trigram)");

    // Trigram tokenizer — may fail if the bundled FTS5 does not include
    // `tokenize='trigram'` support. We log a warning and continue, mirroring
    // the graceful degradation in V11 for the standard FTS5 table.
    let trigram_result = conn.execute_batch(
        "CREATE VIRTUAL TABLE IF NOT EXISTS search_trigram
             USING fts5(segment_id UNINDEXED, content, tokenize='trigram');",
    );
    if let Err(e) = trigram_result {
        warn!("trigram FTS5 table creation skipped (tokenizer not available): {e}");
    }

    conn.execute_batch("INSERT INTO schema_version (version) VALUES (18);")?;

    info!("migration V18 complete: Korean trigram FTS5 table");
    Ok(())
}

pub(super) fn migrate_v19(conn: &Connection) -> Result<(), rusqlite::Error> {
    debug!("migration V19 execution: app_meta key-value table");

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS app_meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        INSERT INTO schema_version (version) VALUES (19);",
    )?;

    info!("migration V19 complete: app_meta table");
    Ok(())
}
