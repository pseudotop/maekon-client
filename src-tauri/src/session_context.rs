//! Assembles system prompt context from local data sources for AI conversation sessions.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{Duration, Utc};
use oneshim_core::config::AppConfig;
use oneshim_core::models::ai_session::{
    ActivitySummary, MessageRole, SessionMessage, SuggestionPatterns, SystemInfo,
    SystemPromptContext, ToolDefinition, UserProfileSummary,
};
use oneshim_core::models::event::Event;
use oneshim_core::ports::session_context_store::SessionContextStorePort;
use tracing::warn;

use crate::scheduler::shared_regime_state::SharedRegimeState;

/// Maximum number of recent events to query for activity summary.
const RECENT_EVENTS_LIMIT: usize = 200;

/// Maximum number of suggestions to query for pattern analysis.
const SUGGESTION_HISTORY_LIMIT: usize = 100;

pub struct SessionContextAssembler {
    storage: Arc<dyn SessionContextStorePort>,
    config: Arc<AppConfig>,
    regime_state: Arc<SharedRegimeState>,
}

impl SessionContextAssembler {
    pub fn new(
        storage: Arc<dyn SessionContextStorePort>,
        config: Arc<AppConfig>,
        regime_state: Arc<SharedRegimeState>,
    ) -> Self {
        Self {
            storage,
            config,
            regime_state,
        }
    }

    pub async fn build_system_prompt(&self) -> SystemPromptContext {
        let (activity, suggestions) = tokio::join!(
            self.query_recent_activity(),
            self.query_suggestion_history(),
        );

        SystemPromptContext {
            user_profile: UserProfileSummary::default(),
            current_regime: self.current_regime(),
            recent_activity: activity,
            suggestion_history: suggestions,
            available_skills: vec![],
            system_info: SystemInfo {
                os: std::env::consts::OS.to_string(),
                active_app: None,
                timezone: Utc::now().format("%Z").to_string(),
            },
        }
    }

    pub async fn build_system_message(&self) -> SessionMessage {
        let context = self.build_system_prompt().await;
        let content = serde_json::to_string_pretty(&context).unwrap_or_else(|_| "{}".to_string());

        SessionMessage {
            role: MessageRole::System,
            content: format!(
                "You are Maekon's AI assistant. Here is the current user context:\n\n{content}"
            ),
            attachments: vec![],
            tools: Some(self.build_tool_definitions()),
            context: None,
            response_format: None,
        }
    }

    /// Build tool definitions for key oneshim-web REST API endpoints.
    ///
    /// Included in the system message so CLI sessions can discover and query
    /// local data (metrics, sessions, events, focus, suggestions).
    fn build_tool_definitions(&self) -> Vec<ToolDefinition> {
        let base = format!("http://localhost:{}/api", self.config.web.port);
        let get = "GET".to_string();
        vec![
            ToolDefinition {
                name: "get_metrics".to_string(),
                description: "Query raw activity metrics".to_string(),
                endpoint: format!("{base}/metrics"),
                method: get.clone(),
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "from": { "type": "string", "description": "ISO-8601 start timestamp" },
                        "to": { "type": "string", "description": "ISO-8601 end timestamp" },
                        "limit": { "type": "integer", "minimum": 1, "maximum": 500 }
                    },
                    "additionalProperties": false
                })),
            },
            ToolDefinition {
                name: "get_stats_summary".to_string(),
                description: "Get summary statistics (app usage, session counts)".to_string(),
                endpoint: format!("{base}/stats/summary"),
                method: get.clone(),
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "date": { "type": "string", "description": "YYYY-MM-DD date override" }
                    },
                    "additionalProperties": false
                })),
            },
            ToolDefinition {
                name: "get_sessions".to_string(),
                description: "List work sessions".to_string(),
                endpoint: format!("{base}/sessions"),
                method: get.clone(),
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                })),
            },
            ToolDefinition {
                name: "get_events".to_string(),
                description: "Query recent activity events".to_string(),
                endpoint: format!("{base}/events"),
                method: get.clone(),
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "from": { "type": "string", "description": "ISO-8601 start timestamp" },
                        "to": { "type": "string", "description": "ISO-8601 end timestamp" },
                        "limit": { "type": "integer", "minimum": 1, "maximum": 500 },
                        "offset": { "type": "integer", "minimum": 0 }
                    },
                    "additionalProperties": false
                })),
            },
            ToolDefinition {
                name: "get_suggestions".to_string(),
                description: "List pending suggestions".to_string(),
                endpoint: format!("{base}/suggestions"),
                method: get.clone(),
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                })),
            },
            ToolDefinition {
                name: "get_focus_metrics".to_string(),
                description: "Get focus and productivity metrics".to_string(),
                endpoint: format!("{base}/focus/metrics"),
                method: get.clone(),
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "from": { "type": "string", "description": "ISO-8601 start timestamp" },
                        "to": { "type": "string", "description": "ISO-8601 end timestamp" }
                    },
                    "additionalProperties": false
                })),
            },
            ToolDefinition {
                name: "search".to_string(),
                description: "Full-text search across events (query param: ?q=...)".to_string(),
                endpoint: format!("{base}/search"),
                method: get,
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "q": { "type": "string", "description": "Full-text search query" },
                        "limit": { "type": "integer", "minimum": 1, "maximum": 200 }
                    },
                    "required": ["q"],
                    "additionalProperties": false
                })),
            },
        ]
    }

    /// Query recent events from storage and summarize into top apps + active/idle minutes.
    ///
    /// Returns `ActivitySummary::default()` on any error.
    async fn query_recent_activity(&self) -> ActivitySummary {
        let now = Utc::now();
        let one_hour_ago = now - Duration::hours(1);

        let events = match self
            .storage
            .get_events(one_hour_ago, now, RECENT_EVENTS_LIMIT)
            .await
        {
            Ok(events) => events,
            Err(err) => {
                warn!("Failed to query recent activity: {err}");
                return ActivitySummary::default();
            }
        };

        if events.is_empty() {
            return ActivitySummary::default();
        }

        // Count app occurrences from User and Context events
        let mut app_counts: HashMap<String, u32> = HashMap::new();
        let mut active_event_count: u32 = 0;

        for event in &events {
            match event {
                Event::User(user_event) => {
                    if !user_event.app_name.is_empty() {
                        *app_counts.entry(user_event.app_name.clone()).or_default() += 1;
                    }
                    active_event_count += 1;
                }
                Event::Context(ctx_event) => {
                    if !ctx_event.app_name.is_empty() {
                        *app_counts.entry(ctx_event.app_name.clone()).or_default() += 1;
                    }
                    active_event_count += 1;
                }
                Event::Input(input_event) => {
                    if !input_event.app_name.is_empty() {
                        *app_counts.entry(input_event.app_name.clone()).or_default() += 1;
                    }
                    active_event_count += 1;
                }
                _ => {}
            }
        }

        // Sort by count descending, take top 5
        let mut sorted_apps: Vec<(String, u32)> = app_counts.into_iter().collect();
        sorted_apps.sort_by_key(|a| std::cmp::Reverse(a.1));
        let top_apps: Vec<String> = sorted_apps
            .into_iter()
            .take(5)
            .map(|(name, _)| name)
            .collect();

        // Estimate active minutes from event density (heuristic: ~3 events per active minute)
        let events_per_minute: u32 = 3;
        let active_minutes = (active_event_count / events_per_minute).min(60);
        let idle_minutes = 60_u32.saturating_sub(active_minutes);

        ActivitySummary {
            top_apps,
            active_minutes,
            idle_minutes,
        }
    }

    /// Query suggestion history from storage and summarize into acceptance patterns.
    ///
    /// Uses `spawn_blocking` because `list_suggestions` is a synchronous SQLite call.
    /// Returns `SuggestionPatterns::default()` on any error.
    async fn query_suggestion_history(&self) -> SuggestionPatterns {
        let storage = self.storage.clone();

        let result =
            tokio::task::spawn_blocking(move || storage.list_suggestions(SUGGESTION_HISTORY_LIMIT))
                .await;

        let records = match result {
            Ok(Ok(records)) => records,
            Ok(Err(err)) => {
                warn!("Failed to query suggestion history: {err}");
                return SuggestionPatterns::default();
            }
            Err(err) => {
                warn!("spawn_blocking join error querying suggestions: {err}");
                return SuggestionPatterns::default();
            }
        };

        let total_received = records.len() as u32;
        let accepted_count = records.iter().filter(|r| r.acted_at.is_some()).count() as u32;
        let rejected_count = records.iter().filter(|r| r.dismissed_at.is_some()).count() as u32;

        SuggestionPatterns {
            total_received,
            accepted_count,
            rejected_count,
        }
    }

    fn current_regime(&self) -> String {
        self.regime_state
            .snapshot()
            .regime_label
            .unwrap_or_else(|| "unknown".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oneshim_storage::sqlite::SqliteStorage;

    #[test]
    fn build_system_message_has_system_role() {
        // SessionContextAssembler requires real dependencies;
        // test the SystemPromptContext serialization separately
        let ctx = SystemPromptContext {
            user_profile: UserProfileSummary::default(),
            current_regime: "deep_work".to_string(),
            recent_activity: ActivitySummary::default(),
            suggestion_history: SuggestionPatterns::default(),
            available_skills: vec![],
            system_info: SystemInfo {
                os: "macos".to_string(),
                active_app: Some("VSCode".to_string()),
                timezone: "KST".to_string(),
            },
        };
        let json = serde_json::to_string_pretty(&ctx).unwrap();
        assert!(json.contains("deep_work"));
        assert!(json.contains("VSCode"));
        assert!(json.contains("KST"));
    }

    #[tokio::test]
    async fn query_recent_activity_returns_default_on_empty_storage() {
        let storage = Arc::new(SqliteStorage::open_in_memory(30).unwrap());
        let config = Arc::new(AppConfig::default_config());
        let regime_state = Arc::new(SharedRegimeState::new());

        let assembler = SessionContextAssembler::new(storage, config, regime_state);
        let activity = assembler.query_recent_activity().await;

        assert!(activity.top_apps.is_empty());
        assert_eq!(activity.active_minutes, 0);
        // Default ActivitySummary has idle_minutes = 0 (empty storage, no window to estimate)
        assert_eq!(activity.idle_minutes, 0);
    }

    #[tokio::test]
    async fn query_suggestion_history_returns_default_on_empty_storage() {
        let storage = Arc::new(SqliteStorage::open_in_memory(30).unwrap());
        let config = Arc::new(AppConfig::default_config());
        let regime_state = Arc::new(SharedRegimeState::new());

        let assembler = SessionContextAssembler::new(storage, config, regime_state);
        let patterns = assembler.query_suggestion_history().await;

        assert_eq!(patterns.total_received, 0);
        assert_eq!(patterns.accepted_count, 0);
        assert_eq!(patterns.rejected_count, 0);
    }

    #[tokio::test]
    async fn build_system_prompt_includes_regime() {
        let storage = Arc::new(SqliteStorage::open_in_memory(30).unwrap());
        let config = Arc::new(AppConfig::default_config());
        let regime_state = Arc::new(SharedRegimeState::new());

        let assembler = SessionContextAssembler::new(storage, config, regime_state);
        let prompt = assembler.build_system_prompt().await;

        // Default regime is "unknown" when no regime is set
        assert_eq!(prompt.current_regime, "unknown");
        assert!(prompt.recent_activity.top_apps.is_empty());
        assert_eq!(prompt.suggestion_history.total_received, 0);
    }

    #[tokio::test]
    async fn build_system_message_serializes_context() {
        let storage = Arc::new(SqliteStorage::open_in_memory(30).unwrap());
        let config = Arc::new(AppConfig::default_config());
        let regime_state = Arc::new(SharedRegimeState::new());

        let assembler = SessionContextAssembler::new(storage, config, regime_state);
        let message = assembler.build_system_message().await;

        assert!(matches!(message.role, MessageRole::System));
        assert!(message.content.contains("Maekon's AI assistant"));
        assert!(message.content.contains("unknown")); // default regime
        assert!(message.tools.is_some());
        let tools = message.tools.unwrap();
        assert!(!tools.is_empty());
        assert!(tools.iter().any(|t| t.name == "get_metrics"));
        assert!(tools.iter().all(|t| t.endpoint.contains("localhost")));
    }
}
