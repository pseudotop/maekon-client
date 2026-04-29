//! Pomodoro focus timer session model.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Status of a Pomodoro session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PomodoroStatus {
    /// Work phase is active.
    Running,
    /// Break phase is active.
    OnBreak,
    /// Session finished (work + break).
    Completed,
    /// Session was cancelled before completion.
    Cancelled,
}

/// A single Pomodoro focus timer session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PomodoroSession {
    /// Unique session identifier.
    pub id: String,
    /// When the session was started.
    pub started_at: DateTime<Utc>,
    /// Work duration in minutes (default 25).
    pub duration_minutes: u32,
    /// Break duration in minutes (default 5).
    pub break_minutes: u32,
    /// Current session status.
    pub status: PomodoroStatus,
    /// When the session was completed or cancelled.
    pub completed_at: Option<DateTime<Utc>>,
}

impl PomodoroSession {
    /// Create a new running Pomodoro session.
    pub fn new(id: String, duration_minutes: u32, break_minutes: u32) -> Self {
        Self {
            id,
            started_at: Utc::now(),
            duration_minutes,
            break_minutes,
            status: PomodoroStatus::Running,
            completed_at: None,
        }
    }

    /// Elapsed seconds since the session started.
    pub fn elapsed_secs(&self) -> i64 {
        (Utc::now() - self.started_at).num_seconds()
    }

    /// Total work phase duration in seconds.
    pub fn work_secs(&self) -> i64 {
        self.duration_minutes as i64 * 60
    }

    /// Total break phase duration in seconds.
    pub fn break_secs(&self) -> i64 {
        self.break_minutes as i64 * 60
    }

    /// Remaining seconds in the current phase. Returns 0 when complete.
    pub fn remaining_secs(&self) -> i64 {
        let elapsed = self.elapsed_secs();
        match self.status {
            PomodoroStatus::Running => (self.work_secs() - elapsed).max(0),
            PomodoroStatus::OnBreak => {
                let break_elapsed = elapsed - self.work_secs();
                (self.break_secs() - break_elapsed).max(0)
            }
            PomodoroStatus::Completed | PomodoroStatus::Cancelled => 0,
        }
    }

    /// Derive the effective status based on elapsed time.
    /// Transitions Running -> OnBreak -> Completed automatically.
    pub fn effective_status(&self) -> PomodoroStatus {
        match self.status {
            PomodoroStatus::Running => {
                let elapsed = self.elapsed_secs();
                if elapsed >= self.work_secs() + self.break_secs() {
                    PomodoroStatus::Completed
                } else if elapsed >= self.work_secs() {
                    PomodoroStatus::OnBreak
                } else {
                    PomodoroStatus::Running
                }
            }
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_session_defaults() {
        let session = PomodoroSession::new("test-1".to_string(), 25, 5);
        assert_eq!(session.duration_minutes, 25);
        assert_eq!(session.break_minutes, 5);
        assert_eq!(session.status, PomodoroStatus::Running);
        assert!(session.completed_at.is_none());
    }

    #[test]
    fn work_and_break_secs() {
        let session = PomodoroSession::new("test-2".to_string(), 25, 5);
        assert_eq!(session.work_secs(), 1500);
        assert_eq!(session.break_secs(), 300);
    }

    #[test]
    fn effective_status_running() {
        let session = PomodoroSession::new("test-3".to_string(), 25, 5);
        // Just created, should still be Running
        assert_eq!(session.effective_status(), PomodoroStatus::Running);
    }

    #[test]
    fn effective_status_cancelled_stays_cancelled() {
        let mut session = PomodoroSession::new("test-4".to_string(), 25, 5);
        session.status = PomodoroStatus::Cancelled;
        session.completed_at = Some(Utc::now());
        assert_eq!(session.effective_status(), PomodoroStatus::Cancelled);
    }

    #[test]
    fn remaining_secs_when_cancelled() {
        let mut session = PomodoroSession::new("test-5".to_string(), 25, 5);
        session.status = PomodoroStatus::Cancelled;
        assert_eq!(session.remaining_secs(), 0);
    }

    #[test]
    fn serde_roundtrip() {
        let session = PomodoroSession::new("test-6".to_string(), 25, 5);
        let json = serde_json::to_string(&session).unwrap();
        let deserialized: PomodoroSession = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, "test-6");
        assert_eq!(deserialized.status, PomodoroStatus::Running);
    }

    #[test]
    fn status_serde_snake_case() {
        let json = serde_json::to_string(&PomodoroStatus::OnBreak).unwrap();
        assert_eq!(json, r#""on_break""#);
    }
}
