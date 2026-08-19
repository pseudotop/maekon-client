//! Assembles system prompt context from local data sources for AI conversation sessions.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{Duration, Utc};
use maekon_core::config::AppConfig;
use maekon_core::models::ai_session::{
    ActivitySummary, MessageRole, SessionMessage, SuggestionPatterns, SystemInfo,
    SystemPromptContext, ToolDefinition, UserProfileSummary,
};
use maekon_core::models::event::Event;
use maekon_core::ports::session_context_store::SessionContextStorePort;
use tracing::warn;

use crate::scheduler::shared_regime_state::SharedRegimeState;

/// #6266: recursively mask every JSON STRING VALUE in `value` at `level`, in
/// place. Object keys are left untouched (they are field names, not PII).
/// Masking each leaf string individually — rather than the joined serialized
/// document — keeps a PII match within its own JSON string so it cannot consume
/// the closing quote and corrupt the structure. `PiiFilterLevel::Off` makes the
/// masker a no-op, so this is a cheap pass for users who opted out.
fn mask_json_string_values(
    value: &mut serde_json::Value,
    level: maekon_core::config::PiiFilterLevel,
) {
    match value {
        serde_json::Value::String(s) => {
            *s = maekon_vision::privacy::sanitize_title_with_level(s, level);
        }
        serde_json::Value::Array(items) => {
            for item in items.iter_mut() {
                mask_json_string_values(item, level);
            }
        }
        serde_json::Value::Object(map) => {
            for v in map.values_mut() {
                mask_json_string_values(v, level);
            }
        }
        _ => {}
    }
}

fn system_prompt_egress_pii_level(config: &AppConfig) -> maekon_core::config::PiiFilterLevel {
    config
        .ai_provider
        .external_data_policy
        .effective_egress_pii_level(config.privacy.pii_filter_level)
}

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
        // #6266: mask PII in the assembled context (top_apps / window titles /
        // regime / active_app) at the configured level BEFORE it is baked into
        // the system prompt. For external sessions this prompt egresses to the
        // provider, and unlike per-turn user messages it bypasses the conversation
        // privacy guard (GuardedConversationSession sanitizes only send_message
        // content, not the create-time system prompt).
        //
        // Mask each STRING VALUE of the serialized context INDIVIDUALLY (via a
        // serde_json::Value walk), not the flat serialized string: the PII maskers
        // do not respect JSON token boundaries, so masking the joined string could
        // let an email/path span consume a closing `"` and corrupt the structure
        // (#6266 verify). Per-value masking keeps each string self-contained and
        // re-serialization yields valid JSON. Keys are untouched. For external
        // sessions, resolve the same effective egress floor as the per-turn chat
        // path so the create-time system prompt cannot bypass stricter provider
        // policy.
        let mut value = serde_json::to_value(&context).unwrap_or(serde_json::Value::Null);
        mask_json_string_values(&mut value, system_prompt_egress_pii_level(&self.config));
        let content = serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".to_string());

        SessionMessage {
            // #9643 re-review M1: this content IS screen-derived (top apps,
            // window titles, regime). It never reaches the conversation guard
            // today (consumed via config.system_prompt only, with its own
            // masking path), but the label must not hand a future
            // send_message caller the lenient gate.
            screen_derived: true,
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

    /// Build tool definitions for key maekon-web REST API endpoints.
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
    use maekon_storage::sqlite::SqliteStorage;

    #[test]
    fn mask_json_string_values_masks_pii_and_preserves_valid_json() {
        // #6266 regression guard: the previous fix masked the FLAT serialized
        // string, so an email at the end of a value (e.g. a webmail window title)
        // consumed the closing quote and corrupted the JSON. Per-value masking
        // must (a) mask the PII, (b) keep keys intact, and (c) re-serialize to
        // VALID JSON (re-parseable) — exactly the case the old approach broke.
        let mut value = serde_json::json!({
            "current_regime": "deep_work",
            "system_info": { "active_app": "Mail - alice@example.com", "os": "macos" },
            "recent_activity": { "top_apps": ["Slack — bob@example.com", "VSCode"] },
        });
        mask_json_string_values(&mut value, maekon_core::config::PiiFilterLevel::Standard);

        // (c) Re-serializes to valid JSON (the old flat-string masking did not).
        let serialized = serde_json::to_string(&value).expect("masked value serializes");
        let reparsed: serde_json::Value =
            serde_json::from_str(&serialized).expect("masked context must remain valid JSON");

        // (a) PII masked, raw addresses gone.
        assert!(
            !serialized.contains("alice@example.com"),
            "email must be masked"
        );
        assert!(
            !serialized.contains("bob@example.com"),
            "list email must be masked"
        );
        // (b) keys + non-PII values preserved, structure intact.
        assert_eq!(reparsed["system_info"]["os"], "macos");
        assert_eq!(reparsed["current_regime"], "deep_work");
        assert_eq!(reparsed["recent_activity"]["top_apps"][1], "VSCode");
        assert!(reparsed["system_info"]["active_app"]
            .as_str()
            .expect("active_app stays a string")
            .starts_with("Mail - "));
    }

    #[test]
    fn mask_json_string_values_off_level_is_noop() {
        let mut value = serde_json::json!({ "active_app": "Mail - alice@example.com" });
        mask_json_string_values(&mut value, maekon_core::config::PiiFilterLevel::Off);
        assert_eq!(value["active_app"], "Mail - alice@example.com");
    }

    #[test]
    fn system_prompt_egress_level_uses_external_policy_floor() {
        let mut config = AppConfig::default_config();
        config.privacy.pii_filter_level = maekon_core::config::PiiFilterLevel::Off;
        config.ai_provider.external_data_policy =
            maekon_core::config::ExternalDataPolicy::PiiFilterStrict;

        let mut value = serde_json::json!({ "active_app": "Mail - alice@example.com" });
        mask_json_string_values(&mut value, system_prompt_egress_pii_level(&config));

        assert_eq!(
            system_prompt_egress_pii_level(&config),
            maekon_core::config::PiiFilterLevel::Strict
        );
        assert!(
            !value["active_app"]
                .as_str()
                .expect("active_app stays a string")
                .contains("alice@example.com"),
            "system prompt context must follow external egress policy, not base Off"
        );
    }

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
