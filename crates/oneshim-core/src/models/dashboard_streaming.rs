//! Record types used by v2b dashboard gRPC streaming.
//!
//! These are plain data containers. The proto wire types live in
//! `oneshim-web::proto::dashboard::v1::*`; we keep the storage layer free
//! of proto dependencies by going through these intermediate records.

use chrono::{DateTime, Utc};

#[derive(Debug, Clone, PartialEq)]
pub struct MetricBucketRecord {
    pub start: DateTime<Utc>,
    pub cpu_avg_pct: f64,
    pub memory_avg_mb: f64,
    pub active_keystrokes: u32,
    pub active_mouse_clicks: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DashboardEventSignal {
    Frame(i64),      // frames table primary key
    Idle,            // latest-state lookup (served from event payload)
    AiRuntimeStatus, // latest-state lookup (served from event payload)
}

#[derive(Debug, Clone, PartialEq)]
pub enum DashboardEventRecord {
    Frame {
        frame_id: i64,
        occurred_at: DateTime<Utc>,
        app_name: String,
        window_title: String,
        importance: f32,
        trigger_type: String,
    },
    Idle {
        occurred_at: DateTime<Utc>,
        is_idle: bool,
        idle_secs: u64,
    },
    AiRuntimeStatus {
        occurred_at: DateTime<Utc>,
        ocr_source: String,
        llm_source: String,
        ocr_fallback_reason: String, // empty string when no fallback
        llm_fallback_reason: String,
    },
}
