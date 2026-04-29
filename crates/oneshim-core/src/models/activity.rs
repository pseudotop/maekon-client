use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum IdleState {
    Active,
    Idle,
    Locked,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdleInfo {
    pub state: IdleState,
    pub idle_secs: u64,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdlePeriod {
    pub start_time: DateTime<Utc>,
    pub end_time: Option<DateTime<Utc>>,
    pub duration_secs: Option<u64>,
}

impl IdlePeriod {
    pub fn start_now() -> Self {
        Self {
            start_time: Utc::now(),
            end_time: None,
            duration_secs: None,
        }
    }

    pub fn end_now(&mut self) {
        let now = Utc::now();
        self.end_time = Some(now);
        self.duration_secs = Some((now - self.start_time).num_seconds() as u64);
    }

    pub fn is_ongoing(&self) -> bool {
        self.end_time.is_none()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStats {
    pub session_id: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub total_events: u64,
    pub total_frames: u64,
    pub total_idle_secs: u64,
}

impl SessionStats {
    pub fn new(session_id: String) -> Self {
        Self {
            session_id,
            started_at: Utc::now(),
            ended_at: None,
            total_events: 0,
            total_frames: 0,
            total_idle_secs: 0,
        }
    }

    pub fn increment_events(&mut self) {
        self.total_events += 1;
    }

    pub fn increment_frames(&mut self) {
        self.total_frames += 1;
    }

    pub fn add_idle_secs(&mut self, secs: u64) {
        self.total_idle_secs += secs;
    }

    pub fn end_now(&mut self) {
        self.ended_at = Some(Utc::now());
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessSnapshot {
    pub timestamp: DateTime<Utc>,
    pub processes: Vec<ProcessSnapshotEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessSnapshotEntry {
    pub pid: u32,
    pub name: String,
    pub cpu_usage: f32,
    pub memory_bytes: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_period_lifecycle() {
        let mut period = IdlePeriod::start_now();
        assert!(period.is_ongoing());
        assert!(period.end_time.is_none());

        std::thread::sleep(std::time::Duration::from_millis(10));
        period.end_now();

        assert!(!period.is_ongoing());
        assert!(period.end_time.is_some());
        assert!(period.duration_secs.is_some());
    }

    #[test]
    fn session_stats_counters() {
        let mut stats = SessionStats::new("test-session".to_string());
        assert_eq!(stats.total_events, 0);
        assert_eq!(stats.total_frames, 0);

        stats.increment_events();
        stats.increment_events();
        stats.increment_frames();
        stats.add_idle_secs(30);

        assert_eq!(stats.total_events, 2);
        assert_eq!(stats.total_frames, 1);
        assert_eq!(stats.total_idle_secs, 30);
    }
}
