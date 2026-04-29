use crate::error::StorageError;
use oneshim_core::models::coaching::CoachingEventRow;
use std::collections::HashMap;

use super::SqliteStorage;

impl SqliteStorage {
    /// Query coaching events, newest first, with pagination.
    pub fn query_coaching_events(
        &self,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<CoachingEventRow>, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("lock poisoned: {e}")))?;

        let mut stmt = conn
            .prepare(
                "SELECT event_id, trigger_type, profile_name, regime_id,
                        message_template, personalized_message, shown_at,
                        dismissed_at, dismiss_action, feedback_type, feedback_score
                 FROM coaching_events
                 ORDER BY shown_at DESC
                 LIMIT ?1 OFFSET ?2",
            )
            .map_err(|e| StorageError::Internal(format!("prepare query_coaching_events: {e}")))?;

        let rows = stmt
            .query_map(rusqlite::params![limit, offset], |row| {
                Ok(CoachingEventRow {
                    event_id: row.get(0)?,
                    trigger_type: row.get(1)?,
                    profile_name: row.get(2)?,
                    regime_id: row.get(3)?,
                    message_template: row.get(4)?,
                    personalized_message: row.get(5)?,
                    shown_at: row.get(6)?,
                    dismissed_at: row.get(7)?,
                    dismiss_action: row.get(8)?,
                    feedback_type: row.get(9)?,
                    feedback_score: row.get(10)?,
                })
            })
            .map_err(|e| StorageError::Internal(format!("query_coaching_events: {e}")))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| StorageError::Internal(format!("row read: {e}")))?);
        }
        Ok(results)
    }

    /// Query coaching events shown on or after a given date (YYYY-MM-DD).
    pub fn query_coaching_events_since(
        &self,
        since_date: &str,
    ) -> Result<Vec<CoachingEventRow>, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("lock poisoned: {e}")))?;

        let mut stmt = conn
            .prepare(
                "SELECT event_id, trigger_type, profile_name, regime_id,
                        message_template, personalized_message, shown_at,
                        dismissed_at, dismiss_action, feedback_type, feedback_score
                 FROM coaching_events
                 WHERE shown_at >= ?1
                 ORDER BY shown_at DESC",
            )
            .map_err(|e| {
                StorageError::Internal(format!("prepare query_coaching_events_since: {e}"))
            })?;

        let rows = stmt
            .query_map(rusqlite::params![since_date], |row| {
                Ok(CoachingEventRow {
                    event_id: row.get(0)?,
                    trigger_type: row.get(1)?,
                    profile_name: row.get(2)?,
                    regime_id: row.get(3)?,
                    message_template: row.get(4)?,
                    personalized_message: row.get(5)?,
                    shown_at: row.get(6)?,
                    dismissed_at: row.get(7)?,
                    dismiss_action: row.get(8)?,
                    feedback_type: row.get(9)?,
                    feedback_score: row.get(10)?,
                })
            })
            .map_err(|e| StorageError::Internal(format!("query_coaching_events_since: {e}")))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| StorageError::Internal(format!("row read: {e}")))?);
        }
        Ok(results)
    }

    /// Insert a coaching event record.
    pub fn insert_coaching_event(&self, event: &CoachingEventRow) -> Result<(), StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("lock poisoned: {e}")))?;

        conn.execute(
            "INSERT INTO coaching_events
                (event_id, trigger_type, profile_name, regime_id,
                 message_template, personalized_message, shown_at,
                 dismissed_at, dismiss_action, feedback_type, feedback_score)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            rusqlite::params![
                event.event_id,
                event.trigger_type,
                event.profile_name,
                event.regime_id,
                event.message_template,
                event.personalized_message,
                event.shown_at,
                event.dismissed_at,
                event.dismiss_action,
                event.feedback_type,
                event.feedback_score,
            ],
        )
        .map_err(|e| StorageError::Internal(format!("insert_coaching_event: {e}")))?;

        Ok(())
    }

    /// Update coaching event with dismiss/feedback data.
    pub fn update_coaching_event_feedback(
        &self,
        event_id: &str,
        dismiss_action: Option<&str>,
        dismissed_at: Option<&str>,
        feedback_type: Option<&str>,
        feedback_score: Option<f64>,
    ) -> Result<(), StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("lock poisoned: {e}")))?;

        conn.execute(
            "UPDATE coaching_events
             SET dismiss_action = COALESCE(?2, dismiss_action),
                 dismissed_at = COALESCE(?3, dismissed_at),
                 feedback_type = COALESCE(?4, feedback_type),
                 feedback_score = COALESCE(?5, feedback_score)
             WHERE event_id = ?1",
            rusqlite::params![
                event_id,
                dismiss_action,
                dismissed_at,
                feedback_type,
                feedback_score
            ],
        )
        .map_err(|e| StorageError::Internal(format!("update_coaching_event_feedback: {e}")))?;

        Ok(())
    }

    /// Update the personalized message text for a coaching event.
    pub fn update_coaching_event_personalized(
        &self,
        event_id: &str,
        personalized_text: &str,
    ) -> Result<(), StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("lock poisoned: {e}")))?;

        conn.execute(
            "UPDATE coaching_events SET personalized_message = ?2 WHERE event_id = ?1",
            rusqlite::params![event_id, personalized_text],
        )
        .map_err(|e| StorageError::Internal(format!("update_coaching_event_personalized: {e}")))?;

        Ok(())
    }

    /// Get all regime goals from the regime_goals table.
    pub fn get_regime_goals(&self) -> Result<HashMap<String, u32>, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("lock poisoned: {e}")))?;

        let mut stmt = conn
            .prepare("SELECT regime_label, daily_target_minutes FROM regime_goals")
            .map_err(|e| StorageError::Internal(format!("prepare get_regime_goals: {e}")))?;

        let rows = stmt
            .query_map([], |row| {
                let label: String = row.get(0)?;
                let minutes: u32 = row.get(1)?;
                Ok((label, minutes))
            })
            .map_err(|e| StorageError::Internal(format!("get_regime_goals: {e}")))?;

        let mut goals = HashMap::new();
        for row in rows {
            let (label, minutes) =
                row.map_err(|e| StorageError::Internal(format!("row read: {e}")))?;
            goals.insert(label, minutes);
        }
        Ok(goals)
    }

    /// Insert or update a single regime goal.
    pub fn set_regime_goal(
        &self,
        regime_label: &str,
        target_minutes: u32,
    ) -> Result<(), StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("lock poisoned: {e}")))?;

        conn.execute(
            "INSERT INTO regime_goals (regime_label, daily_target_minutes, updated_at)
             VALUES (?1, ?2, datetime('now'))
             ON CONFLICT(regime_label)
             DO UPDATE SET daily_target_minutes = ?2, updated_at = datetime('now')",
            rusqlite::params![regime_label, target_minutes],
        )
        .map_err(|e| StorageError::Internal(format!("set_regime_goal: {e}")))?;

        Ok(())
    }

    /// Delete a regime goal by label.
    pub fn delete_regime_goal(&self, regime_label: &str) -> Result<(), StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("lock poisoned: {e}")))?;

        conn.execute(
            "DELETE FROM regime_goals WHERE regime_label = ?1",
            rusqlite::params![regime_label],
        )
        .map_err(|e| StorageError::Internal(format!("delete_regime_goal: {e}")))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_storage() -> SqliteStorage {
        SqliteStorage::open_in_memory(30).expect("in-memory storage")
    }

    #[test]
    fn insert_and_query_coaching_event() {
        let storage = test_storage();

        let event = CoachingEventRow {
            event_id: "evt-001".to_string(),
            trigger_type: "RegimeOverstay".to_string(),
            profile_name: "DeepWorkCoach".to_string(),
            regime_id: Some("deep-work".to_string()),
            message_template: "Take a break.".to_string(),
            personalized_message: None,
            shown_at: "2026-03-20T10:00:00Z".to_string(),
            dismissed_at: None,
            dismiss_action: None,
            feedback_type: None,
            feedback_score: None,
        };

        storage.insert_coaching_event(&event).unwrap();

        let results = storage.query_coaching_events(10, 0).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].event_id, "evt-001");
        assert_eq!(results[0].trigger_type, "RegimeOverstay");
        assert_eq!(results[0].profile_name, "DeepWorkCoach");
    }

    #[test]
    fn query_coaching_events_pagination() {
        let storage = test_storage();

        for i in 0..5 {
            let event = CoachingEventRow {
                event_id: format!("evt-{:03}", i),
                trigger_type: "RegimeDrift".to_string(),
                profile_name: "FocusGuard".to_string(),
                regime_id: None,
                message_template: format!("Message {i}"),
                personalized_message: None,
                shown_at: format!("2026-03-20T10:{:02}:00Z", i),
                dismissed_at: None,
                dismiss_action: None,
                feedback_type: None,
                feedback_score: None,
            };
            storage.insert_coaching_event(&event).unwrap();
        }

        let page = storage.query_coaching_events(2, 2).unwrap();
        assert_eq!(page.len(), 2);
    }

    #[test]
    fn set_and_get_regime_goals() {
        let storage = test_storage();

        storage.set_regime_goal("Deep Work", 120).unwrap();
        storage.set_regime_goal("Communication", 60).unwrap();
        storage.set_regime_goal("Email", 30).unwrap();

        let goals = storage.get_regime_goals().unwrap();
        assert_eq!(goals.len(), 3);
        assert_eq!(goals["Deep Work"], 120);
        assert_eq!(goals["Communication"], 60);
        assert_eq!(goals["Email"], 30);
    }

    #[test]
    fn update_regime_goal_overwrites() {
        let storage = test_storage();

        storage.set_regime_goal("Deep Work", 120).unwrap();
        storage.set_regime_goal("Deep Work", 180).unwrap();

        let goals = storage.get_regime_goals().unwrap();
        assert_eq!(goals.len(), 1);
        assert_eq!(goals["Deep Work"], 180);
    }

    #[test]
    fn delete_regime_goal_removes() {
        let storage = test_storage();

        storage.set_regime_goal("Deep Work", 120).unwrap();
        storage.set_regime_goal("Email", 30).unwrap();

        storage.delete_regime_goal("Deep Work").unwrap();

        let goals = storage.get_regime_goals().unwrap();
        assert_eq!(goals.len(), 1);
        assert!(!goals.contains_key("Deep Work"));
    }

    #[test]
    fn update_coaching_event_feedback_updates_fields() {
        let storage = test_storage();

        let event = CoachingEventRow {
            event_id: "evt-fb-001".to_string(),
            trigger_type: "GoalThreshold".to_string(),
            profile_name: "GoalTracker".to_string(),
            regime_id: None,
            message_template: "Great progress!".to_string(),
            personalized_message: None,
            shown_at: "2026-03-20T12:00:00Z".to_string(),
            dismissed_at: None,
            dismiss_action: None,
            feedback_type: None,
            feedback_score: None,
        };

        storage.insert_coaching_event(&event).unwrap();

        storage
            .update_coaching_event_feedback(
                "evt-fb-001",
                Some("ok"),
                Some("2026-03-20T12:00:15Z"),
                Some("ExplicitPositive"),
                Some(1.0),
            )
            .unwrap();

        let results = storage.query_coaching_events(10, 0).unwrap();
        assert_eq!(results[0].dismiss_action.as_deref(), Some("ok"));
        assert_eq!(
            results[0].feedback_type.as_deref(),
            Some("ExplicitPositive")
        );
    }
}
