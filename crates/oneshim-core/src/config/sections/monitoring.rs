// 모니터링/스케줄 설정 — 시스템 감시, 화면 캡처, 활성 시간, 파일 접근 설정
use super::super::enums::Weekday;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ── MonitorConfig ──────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitorConfig {
    #[serde(default = "default_poll_interval_ms")]
    pub poll_interval_ms: u64,
    #[serde(default = "default_sync_interval_ms")]
    pub sync_interval_ms: u64,
    #[serde(default = "default_heartbeat_interval_ms")]
    pub heartbeat_interval_ms: u64,
    #[serde(default = "default_idle_threshold_secs")]
    pub idle_threshold_secs: u64,
    #[serde(default = "default_process_interval_secs")]
    pub process_interval_secs: u64,
    #[serde(default = "default_true")]
    pub process_monitoring: bool,
    #[serde(default = "default_true")]
    pub input_activity: bool,
    /// Enable server upload of collected events. Default: false (safe).
    #[serde(default)]
    pub upload_enabled: bool,
}

// ── VisionConfig ───────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisionConfig {
    #[serde(default = "default_capture_enabled")]
    pub capture_enabled: bool,
    #[serde(default = "default_capture_throttle_ms")]
    pub capture_throttle_ms: u64,
    #[serde(default = "default_thumbnail_width")]
    pub thumbnail_width: u32,
    #[serde(default = "default_thumbnail_height")]
    pub thumbnail_height: u32,
    #[serde(default)]
    pub ocr_enabled: bool,
    #[serde(default)]
    pub privacy_mode: bool,
}

impl VisionConfig {
    /// Validate that vision configuration values are within acceptable bounds.
    pub fn validate_bounds(&self) -> Result<(), String> {
        if self.capture_throttle_ms < 100 {
            return Err("vision.capture_throttle_ms must be >= 100".to_string());
        }
        Ok(())
    }
}

// ── ScheduleConfig ─────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleConfig {
    #[serde(default)]
    pub active_hours_enabled: bool,
    #[serde(default = "default_active_start_hour")]
    pub active_start_hour: u8,
    #[serde(default = "default_active_end_hour")]
    pub active_end_hour: u8,
    #[serde(default = "default_active_days")]
    pub active_days: Vec<Weekday>,
    #[serde(default = "default_true")]
    pub pause_on_screen_lock: bool,
    #[serde(default)]
    pub pause_on_battery_saver: bool,
}

impl Default for ScheduleConfig {
    fn default() -> Self {
        Self {
            active_hours_enabled: false,
            active_start_hour: default_active_start_hour(),
            active_end_hour: default_active_end_hour(),
            active_days: default_active_days(),
            pause_on_screen_lock: true,
            pause_on_battery_saver: false,
        }
    }
}

// ── FileAccessConfig ───────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileAccessConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub monitored_folders: Vec<PathBuf>,
    #[serde(default = "default_excluded_extensions")]
    pub excluded_extensions: Vec<String>,
    #[serde(default = "default_max_events_per_minute")]
    pub max_events_per_minute: u32,
}

impl Default for FileAccessConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            monitored_folders: Vec::new(),
            excluded_extensions: default_excluded_extensions(),
            max_events_per_minute: default_max_events_per_minute(),
        }
    }
}

// ── Default / helper functions (pub(super) — config/mod.rs 에서 사용) ─

pub(crate) fn default_poll_interval_ms() -> u64 {
    1_000
}

pub(crate) fn default_sync_interval_ms() -> u64 {
    10_000
}

pub(crate) fn default_heartbeat_interval_ms() -> u64 {
    30_000
}

pub(crate) fn default_idle_threshold_secs() -> u64 {
    300 // 5 min
}

pub(crate) fn default_process_interval_secs() -> u64 {
    10
}

pub(crate) fn default_capture_enabled() -> bool {
    true
}

pub(crate) fn default_capture_throttle_ms() -> u64 {
    5_000
}

pub(crate) fn default_thumbnail_width() -> u32 {
    480
}

pub(crate) fn default_thumbnail_height() -> u32 {
    270
}

// ── Private default helpers ─────────────────────────────────────────

fn default_true() -> bool {
    true
}

fn default_active_start_hour() -> u8 {
    9
}

fn default_active_end_hour() -> u8 {
    18
}

fn default_active_days() -> Vec<Weekday> {
    vec![
        Weekday::Mon,
        Weekday::Tue,
        Weekday::Wed,
        Weekday::Thu,
        Weekday::Fri,
    ]
}

fn default_excluded_extensions() -> Vec<String> {
    vec![
        ".tmp".to_string(),
        ".log".to_string(),
        ".lock".to_string(),
        ".swp".to_string(),
    ]
}

fn default_max_events_per_minute() -> u32 {
    100
}
