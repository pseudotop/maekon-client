//! SQLite implementation of `SessionStoragePort` for AI chat session persistence.

use async_trait::async_trait;
use chrono::{DateTime, NaiveDateTime, Utc};

use oneshim_core::error::CoreError;
use oneshim_core::models::ai_session::{
    MessageRecord, SessionRecord, SessionState, SessionTransport,
};
use oneshim_core::ports::session_storage::SessionStoragePort;

use super::SqliteStorage;
use crate::error::StorageError;
use tracing::debug;

/// Max thinking content length persisted (10 KB).
const MAX_THINKING_LEN: usize = 10_240;

/// Parse an ISO 8601 datetime string from SQLite into `DateTime<Utc>`.
fn parse_dt(s: &str) -> DateTime<Utc> {
    NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
        .map(|n| n.and_utc())
        .or_else(|_| DateTime::parse_from_rfc3339(s).map(|d| d.with_timezone(&Utc)))
        .unwrap_or_else(|e| {
            tracing::warn!("failed to parse datetime '{s}': {e}, using Utc::now()");
            Utc::now()
        })
}

/// Format `DateTime<Utc>` for SQLite TEXT storage.
fn fmt_dt(dt: &DateTime<Utc>) -> String {
    dt.format("%Y-%m-%d %H:%M:%S").to_string()
}

/// Parse a `SessionState` from its serde snake_case string.
fn parse_state(s: &str) -> SessionState {
    serde_json::from_value(serde_json::Value::String(s.to_string()))
        .unwrap_or(SessionState::Terminated)
}

/// Parse a `SessionTransport` from its serde snake_case string.
fn parse_transport(s: &str) -> SessionTransport {
    serde_json::from_value(serde_json::Value::String(s.to_string()))
        .unwrap_or(SessionTransport::HttpApi)
}

/// Serialize an enum to its serde snake_case string.
fn enum_str<T: serde::Serialize>(val: &T) -> String {
    serde_json::to_value(val)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default()
}

#[async_trait]
impl SessionStoragePort for SqliteStorage {
    async fn save_session(&self, record: &SessionRecord) -> Result<(), CoreError> {
        let r = record.clone();
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT OR REPLACE INTO ai_sessions
                 (session_id, provider, model, transport, state, system_prompt,
                  turn_count, total_input_tokens, total_output_tokens,
                  created_at, last_active, terminated_at, title)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                rusqlite::params![
                    r.session_id,
                    r.provider_name,
                    r.model,
                    enum_str(&r.transport),
                    enum_str(&r.state),
                    r.system_prompt,
                    r.turn_count,
                    r.total_input_tokens as i64,
                    r.total_output_tokens as i64,
                    fmt_dt(&r.created_at),
                    fmt_dt(&r.last_active),
                    r.terminated_at.as_ref().map(fmt_dt),
                    r.title,
                ],
            )
            .map_err(StorageError::Sqlite)?;
            Ok(())
        })
        .await
        .map_err(CoreError::from)
    }

    async fn update_session_state(
        &self,
        session_id: &str,
        state: &SessionState,
    ) -> Result<(), CoreError> {
        let sid = session_id.to_string();
        let state_str = enum_str(state);
        let is_terminated = *state == SessionState::Terminated;
        self.with_conn(move |conn| {
            if is_terminated {
                conn.execute(
                    "UPDATE ai_sessions SET state = ?1, terminated_at = datetime('now')
                     WHERE session_id = ?2",
                    rusqlite::params![state_str, sid],
                )
                .map_err(StorageError::Sqlite)?;
            } else {
                conn.execute(
                    "UPDATE ai_sessions SET state = ?1, last_active = datetime('now')
                     WHERE session_id = ?2",
                    rusqlite::params![state_str, sid],
                )
                .map_err(StorageError::Sqlite)?;
            }
            Ok(())
        })
        .await
        .map_err(CoreError::from)
    }

    async fn terminate_session(&self, session_id: &str) -> Result<(), CoreError> {
        self.update_session_state(session_id, &SessionState::Terminated)
            .await
    }

    async fn update_session_usage(
        &self,
        session_id: &str,
        input_tokens: u64,
        output_tokens: u64,
    ) -> Result<(), CoreError> {
        let sid = session_id.to_string();
        self.with_conn(move |conn| {
            conn.execute(
                "UPDATE ai_sessions
                 SET total_input_tokens = total_input_tokens + ?1,
                     total_output_tokens = total_output_tokens + ?2,
                     turn_count = turn_count + 1, last_active = datetime('now')
                 WHERE session_id = ?3",
                rusqlite::params![input_tokens as i64, output_tokens as i64, sid],
            )
            .map_err(StorageError::Sqlite)?;
            Ok(())
        })
        .await
        .map_err(CoreError::from)
    }

    async fn list_sessions(&self, limit: u32) -> Result<Vec<SessionRecord>, CoreError> {
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT session_id, provider, model, transport, state, system_prompt,
                            turn_count, total_input_tokens, total_output_tokens,
                            created_at, last_active, terminated_at, title
                     FROM ai_sessions
                     ORDER BY last_active DESC
                     LIMIT ?1",
                )
                .map_err(StorageError::Sqlite)?;

            let rows = stmt
                .query_map([limit], |row| {
                    Ok(SessionRecord {
                        session_id: row.get(0)?,
                        provider_name: row.get(1)?,
                        model: row.get(2)?,
                        transport: parse_transport(&row.get::<_, String>(3)?),
                        state: parse_state(&row.get::<_, String>(4)?),
                        system_prompt: row.get(5)?,
                        turn_count: row.get(6)?,
                        total_input_tokens: row.get::<_, i64>(7)? as u64,
                        total_output_tokens: row.get::<_, i64>(8)? as u64,
                        created_at: parse_dt(&row.get::<_, String>(9)?),
                        last_active: parse_dt(&row.get::<_, String>(10)?),
                        terminated_at: row.get::<_, Option<String>>(11)?.map(|s| parse_dt(&s)),
                        title: row.get(12)?,
                    })
                })
                .map_err(StorageError::Sqlite)?;

            rows.collect::<Result<Vec<_>, _>>()
                .map_err(StorageError::Sqlite)
        })
        .await
        .map_err(CoreError::from)
    }

    async fn save_messages(
        &self,
        session_id: &str,
        messages: &[MessageRecord],
    ) -> Result<(), CoreError> {
        let sid = session_id.to_string();
        let msgs: Vec<MessageRecord> = messages.to_vec();
        self.with_conn(move |conn| {
            conn.execute_batch("BEGIN IMMEDIATE")
                .map_err(StorageError::Sqlite)?;
            let result = (|| -> Result<(), StorageError> {
                let mut stmt = conn
                    .prepare(
                        "INSERT INTO ai_conversation_messages
                         (session_id, role, content, thinking, tool_use,
                          usage_input, usage_output, created_at, seq)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    )
                    .map_err(StorageError::Sqlite)?;

                for msg in &msgs {
                    let thinking = msg.thinking.as_ref().map(|t| {
                        if t.len() > MAX_THINKING_LEN {
                            format!("{}... [truncated]", &t[..MAX_THINKING_LEN])
                        } else {
                            t.clone()
                        }
                    });
                    stmt.execute(rusqlite::params![
                        sid,
                        msg.role,
                        msg.content,
                        thinking,
                        msg.tool_use,
                        msg.usage_input.map(|v| v as i64),
                        msg.usage_output.map(|v| v as i64),
                        fmt_dt(&msg.created_at),
                        msg.seq,
                    ])
                    .map_err(StorageError::Sqlite)?;
                }
                Ok(())
            })();
            match result {
                Ok(()) => {
                    conn.execute_batch("COMMIT").map_err(StorageError::Sqlite)?;
                    Ok(())
                }
                Err(e) => {
                    if let Err(e) = conn.execute_batch("ROLLBACK") {
                        debug!("execute_batch failed: {e}");
                    }
                    Err(e)
                }
            }
        })
        .await
        .map_err(CoreError::from)
    }

    async fn load_messages(
        &self,
        session_id: &str,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<MessageRecord>, CoreError> {
        let sid = session_id.to_string();
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, session_id, role, content, thinking, tool_use,
                            usage_input, usage_output, created_at, seq
                     FROM ai_conversation_messages
                     WHERE session_id = ?1
                     ORDER BY seq ASC
                     LIMIT ?2 OFFSET ?3",
                )
                .map_err(StorageError::Sqlite)?;

            let rows = stmt
                .query_map(rusqlite::params![sid, limit, offset], |row| {
                    Ok(MessageRecord {
                        id: row.get(0)?,
                        session_id: row.get(1)?,
                        role: row.get(2)?,
                        content: row.get(3)?,
                        thinking: row.get(4)?,
                        tool_use: row.get(5)?,
                        usage_input: row.get::<_, Option<i64>>(6)?.map(|v| v as u64),
                        usage_output: row.get::<_, Option<i64>>(7)?.map(|v| v as u64),
                        created_at: parse_dt(&row.get::<_, String>(8)?),
                        seq: row.get(9)?,
                    })
                })
                .map_err(StorageError::Sqlite)?;

            rows.collect::<Result<Vec<_>, _>>()
                .map_err(StorageError::Sqlite)
        })
        .await
        .map_err(CoreError::from)
    }

    async fn delete_session(&self, session_id: &str) -> Result<(), CoreError> {
        let sid = session_id.to_string();
        self.with_conn(move |conn| {
            conn.execute_batch("PRAGMA foreign_keys = ON;")
                .map_err(StorageError::Sqlite)?;
            conn.execute("DELETE FROM ai_sessions WHERE session_id = ?1", [&sid])
                .map_err(StorageError::Sqlite)?;
            Ok(())
        })
        .await
        .map_err(CoreError::from)
    }

    async fn purge_expired(&self, retention_days: u32) -> Result<u32, CoreError> {
        let days = retention_days;
        self.with_conn(move |conn| {
            conn.execute_batch("PRAGMA foreign_keys = ON;")
                .map_err(StorageError::Sqlite)?;

            // 1. Terminated sessions past retention
            let deleted1: usize = conn
                .execute(
                    "DELETE FROM ai_sessions
                     WHERE terminated_at IS NOT NULL
                       AND terminated_at < datetime('now', '-' || ?1 || ' days')",
                    [days],
                )
                .map_err(StorageError::Sqlite)?;

            // 2. Orphaned active sessions (crash recovery)
            let deleted2: usize = conn
                .execute(
                    "DELETE FROM ai_sessions
                     WHERE terminated_at IS NULL
                       AND state IN ('active', 'idle', 'starting', 'recovering', 'failed')
                       AND last_active < datetime('now', '-' || ?1 || ' days')",
                    [days * 2],
                )
                .map_err(StorageError::Sqlite)?;

            Ok((deleted1 + deleted2) as u32)
        })
        .await
        .map_err(CoreError::from)
    }

    async fn next_seq(&self, session_id: &str) -> Result<i64, CoreError> {
        let sid = session_id.to_string();
        self.with_conn(move |conn| {
            let seq: i64 = conn
                .query_row(
                    "SELECT COALESCE(MAX(seq), -1) + 1 FROM ai_conversation_messages
                     WHERE session_id = ?1",
                    [&sid],
                    |row| row.get(0),
                )
                .map_err(StorageError::Sqlite)?;
            Ok(seq)
        })
        .await
        .map_err(CoreError::from)
    }

    async fn update_session_title(&self, session_id: &str, title: &str) -> Result<(), CoreError> {
        let sid = session_id.to_string();
        let t = title.to_string();
        self.with_conn(move |conn| {
            conn.execute(
                "UPDATE ai_sessions SET title = ?1 WHERE session_id = ?2",
                rusqlite::params![t, sid],
            )
            .map_err(StorageError::Sqlite)?;
            Ok(())
        })
        .await
        .map_err(CoreError::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup() -> SqliteStorage {
        SqliteStorage::open_in_memory(30).unwrap()
    }

    fn make_session(id: &str) -> SessionRecord {
        SessionRecord {
            session_id: id.to_string(),
            provider_name: "test-provider".to_string(),
            model: "test-model".to_string(),
            transport: SessionTransport::HttpApi,
            state: SessionState::Active,
            system_prompt: None,
            turn_count: 0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            created_at: Utc::now(),
            last_active: Utc::now(),
            terminated_at: None,
            title: None,
        }
    }

    fn make_message(session_id: &str, role: &str, content: &str, seq: i64) -> MessageRecord {
        MessageRecord {
            id: None,
            session_id: session_id.to_string(),
            role: role.to_string(),
            content: content.to_string(),
            thinking: None,
            tool_use: None,
            usage_input: None,
            usage_output: None,
            created_at: Utc::now(),
            seq,
        }
    }

    #[tokio::test]
    async fn save_and_list_sessions() {
        let storage = setup().await;
        storage.save_session(&make_session("s1")).await.unwrap();
        storage.save_session(&make_session("s2")).await.unwrap();

        let sessions = storage.list_sessions(10).await.unwrap();
        assert_eq!(sessions.len(), 2);
    }

    #[tokio::test]
    async fn terminate_and_list() {
        let storage = setup().await;
        storage.save_session(&make_session("s1")).await.unwrap();
        storage.terminate_session("s1").await.unwrap();

        let sessions = storage.list_sessions(10).await.unwrap();
        assert_eq!(sessions[0].state, SessionState::Terminated);
        assert!(sessions[0].terminated_at.is_some());
    }

    #[tokio::test]
    async fn save_and_load_messages() {
        let storage = setup().await;
        storage.save_session(&make_session("s1")).await.unwrap();

        let msgs = vec![
            make_message("s1", "user", "hello", 0),
            make_message("s1", "assistant", "hi there", 1),
        ];
        storage.save_messages("s1", &msgs).await.unwrap();

        let loaded = storage.load_messages("s1", 100, 0).await.unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].role, "user");
        assert_eq!(loaded[0].content, "hello");
        assert_eq!(loaded[1].role, "assistant");
    }

    #[tokio::test]
    async fn load_messages_pagination() {
        let storage = setup().await;
        storage.save_session(&make_session("s1")).await.unwrap();

        let msgs: Vec<MessageRecord> = (0..5)
            .map(|i| make_message("s1", "user", &format!("msg-{i}"), i))
            .collect();
        storage.save_messages("s1", &msgs).await.unwrap();

        let page1 = storage.load_messages("s1", 2, 0).await.unwrap();
        assert_eq!(page1.len(), 2);
        assert_eq!(page1[0].seq, 0);

        let page2 = storage.load_messages("s1", 2, 2).await.unwrap();
        assert_eq!(page2.len(), 2);
        assert_eq!(page2[0].seq, 2);
    }

    #[tokio::test]
    async fn delete_session_cascades() {
        let storage = setup().await;
        storage.save_session(&make_session("s1")).await.unwrap();
        storage
            .save_messages("s1", &[make_message("s1", "user", "hello", 0)])
            .await
            .unwrap();

        storage.delete_session("s1").await.unwrap();

        let sessions = storage.list_sessions(10).await.unwrap();
        assert!(sessions.is_empty());

        let msgs = storage.load_messages("s1", 10, 0).await.unwrap();
        assert!(msgs.is_empty());
    }

    #[tokio::test]
    async fn next_seq_empty_session() {
        let storage = setup().await;
        storage.save_session(&make_session("s1")).await.unwrap();

        let seq = storage.next_seq("s1").await.unwrap();
        assert_eq!(seq, 0);
    }

    #[tokio::test]
    async fn next_seq_after_messages() {
        let storage = setup().await;
        storage.save_session(&make_session("s1")).await.unwrap();
        storage
            .save_messages(
                "s1",
                &[
                    make_message("s1", "user", "hello", 0),
                    make_message("s1", "assistant", "hi", 1),
                ],
            )
            .await
            .unwrap();

        let seq = storage.next_seq("s1").await.unwrap();
        assert_eq!(seq, 2);
    }

    #[tokio::test]
    async fn update_session_usage() {
        let storage = setup().await;
        storage.save_session(&make_session("s1")).await.unwrap();
        storage.update_session_usage("s1", 100, 200).await.unwrap();

        let sessions = storage.list_sessions(10).await.unwrap();
        assert_eq!(sessions[0].total_input_tokens, 100);
        assert_eq!(sessions[0].total_output_tokens, 200);
        assert_eq!(sessions[0].turn_count, 1); // auto-incremented by SQL

        // Second call should accumulate
        storage.update_session_usage("s1", 50, 30).await.unwrap();
        let sessions = storage.list_sessions(10).await.unwrap();
        assert_eq!(sessions[0].total_input_tokens, 150);
        assert_eq!(sessions[0].total_output_tokens, 230);
        assert_eq!(sessions[0].turn_count, 2);
    }

    #[tokio::test]
    async fn update_session_title() {
        let storage = setup().await;
        storage.save_session(&make_session("s1")).await.unwrap();

        storage.update_session_title("s1", "My Chat").await.unwrap();

        let sessions = storage.list_sessions(10).await.unwrap();
        assert_eq!(sessions[0].title, Some("My Chat".to_string()));
    }

    #[tokio::test]
    async fn thinking_truncation() {
        let storage = setup().await;
        storage.save_session(&make_session("s1")).await.unwrap();

        let long_thinking = "x".repeat(20_000);
        let mut msg = make_message("s1", "assistant", "response", 0);
        msg.thinking = Some(long_thinking);
        storage.save_messages("s1", &[msg]).await.unwrap();

        let loaded = storage.load_messages("s1", 10, 0).await.unwrap();
        let thinking = loaded[0].thinking.as_ref().unwrap();
        assert!(thinking.len() <= MAX_THINKING_LEN + 20); // + "... [truncated]"
        assert!(thinking.ends_with("[truncated]"));
    }
}
