/// Row types returned by the web storage port.
///
/// These structs model rows retrieved from SQLite queries. They live in
/// `oneshim-core` so that the `WebStorage` port trait (also in core) can
/// reference them without pulling in the `oneshim-storage` adapter crate.

#[derive(Debug, Clone)]
pub struct FrameRecord {
    pub id: i64,
    pub timestamp: String,
    pub trigger_type: String,
    pub app_name: String,
    pub window_title: String,
    pub importance: f32,
    pub resolution_w: u32,
    pub resolution_h: u32,
    pub file_path: Option<String>,
    pub ocr_text: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TagRecord {
    pub id: i64,
    pub name: String,
    pub color: String,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct FocusWorkSessionRecord {
    pub id: i64,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub primary_app: String,
    pub category: String,
    pub state: String,
    pub interruption_count: u32,
    pub deep_work_secs: u64,
    pub duration_secs: u64,
}

#[derive(Debug, Clone)]
pub struct FocusInterruptionRecord {
    pub id: i64,
    pub interrupted_at: String,
    pub from_app: String,
    pub from_category: String,
    pub to_app: String,
    pub to_category: String,
    pub resumed_at: Option<String>,
    pub resumed_to_app: Option<String>,
    pub duration_secs: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct LocalSuggestionRecord {
    pub id: i64,
    pub suggestion_type: String,
    pub payload: serde_json::Value,
    pub created_at: String,
    pub shown_at: Option<String>,
    pub dismissed_at: Option<String>,
    pub acted_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct HourlyMetricsRecord {
    pub hour: String,
    pub cpu_avg: f64,
    pub cpu_max: f64,
    pub memory_avg: u64,
    pub memory_max: u64,
    pub sample_count: u64,
}

#[derive(Debug, Clone)]
pub struct StorageStatsSummaryRecord {
    pub frame_count: u64,
    pub event_count: u64,
    pub metric_count: u64,
    pub oldest_data_date: Option<String>,
    pub newest_data_date: Option<String>,
    pub page_count: u64,
    pub page_size: u64,
}

#[derive(Debug, Clone, Default)]
pub struct DeletedRangeCounts {
    pub events_deleted: u64,
    pub frames_deleted: u64,
    pub metrics_deleted: u64,
    pub process_snapshots_deleted: u64,
    pub idle_periods_deleted: u64,
}

#[derive(Debug, Clone)]
pub struct EventExportRecord {
    pub event_id: String,
    pub event_type: String,
    pub timestamp: String,
    pub app_name: Option<String>,
    pub window_title: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MetricExportRecord {
    pub timestamp: String,
    pub cpu_usage: f32,
    pub memory_used: u64,
    pub memory_total: u64,
    pub disk_used: u64,
    pub disk_total: u64,
    pub network_upload: u64,
    pub network_download: u64,
}

#[derive(Debug, Clone)]
pub struct FrameExportRecord {
    pub id: i64,
    pub timestamp: String,
    pub trigger_type: String,
    pub app_name: String,
    pub window_title: String,
    pub importance: f32,
    pub resolution_w: u32,
    pub resolution_h: u32,
    pub ocr_text: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SearchFrameRow {
    pub id: i64,
    pub timestamp: String,
    pub app_name: Option<String>,
    pub window_title: Option<String>,
    pub matched_text: Option<String>,
    pub importance: Option<f32>,
    pub file_path: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SearchEventRow {
    pub event_id: String,
    pub timestamp: String,
    pub app_name: Option<String>,
    pub window_title: Option<String>,
    pub data: Option<String>,
}

#[derive(Debug, Clone)]
pub struct FrameTagLinkRecord {
    pub frame_id: i64,
    pub tag_id: i64,
    pub created_at: String,
}

/// Row from the unified V8 `suggestions` table (both rule-based and LLM sources).
#[derive(Debug, Clone)]
pub struct SuggestionRecord {
    pub id: i64,
    pub suggestion_id: String,
    pub suggestion_type: String,
    pub source: String,
    pub content: String,
    pub priority: String,
    pub confidence_score: f64,
    pub relevance_score: f64,
    pub is_actionable: bool,
    pub reasoning: Option<String>,
    pub shown_at: Option<String>,
    pub dismissed_at: Option<String>,
    pub acted_at: Option<String>,
    pub created_at: String,
    pub expires_at: Option<String>,
    /// RFC3339 timestamp for deferred suggestion resurface time.
    pub resurface_at: Option<String>,
}

impl SuggestionRecord {
    /// Convert a storage record back into a domain `Suggestion`.
    ///
    /// Returns `None` if the `suggestion_type` string does not match a known
    /// variant (SCREAMING_SNAKE_CASE as serialized by serde).
    pub fn try_into_suggestion(self) -> Option<crate::models::suggestion::Suggestion> {
        use crate::models::suggestion::*;

        // Handle both SCREAMING_SNAKE_CASE (serde rename_all) and PascalCase
        // (enum_to_sql_str via serde_json) representations in the database.
        let suggestion_type = match self.suggestion_type.as_str() {
            "WORK_GUIDANCE" | "WorkGuidance" => SuggestionType::WorkGuidance,
            "EMAIL_DRAFT" | "EmailDraft" => SuggestionType::EmailDraft,
            "PRODUCTIVITY_TIP" | "ProductivityTip" => SuggestionType::ProductivityTip,
            "WORKFLOW_OPTIMIZATION" | "WorkflowOptimization" => {
                SuggestionType::WorkflowOptimization
            }
            "CONTEXT_BASED" | "ContextBased" => SuggestionType::ContextBased,
            _ => return None,
        };
        let priority = match self.priority.as_str() {
            "LOW" | "Low" => Priority::Low,
            "HIGH" | "High" => Priority::High,
            "CRITICAL" | "Critical" => Priority::Critical,
            _ => Priority::Medium,
        };
        let source = match self.source.as_str() {
            SuggestionSource::LLM_SERVER_STR | "LlmServer" => SuggestionSource::LlmServer,
            SuggestionSource::LLM_LOCAL_STR | "LlmLocal" => SuggestionSource::LlmLocal,
            _ => SuggestionSource::RuleBased,
        };
        Some(Suggestion {
            suggestion_id: self.suggestion_id,
            suggestion_type,
            content: self.content,
            priority,
            confidence_score: self.confidence_score,
            relevance_score: self.relevance_score,
            is_actionable: self.is_actionable,
            created_at: chrono::DateTime::parse_from_rfc3339(&self.created_at)
                .ok()?
                .with_timezone(&chrono::Utc),
            expires_at: self.expires_at.as_ref().and_then(|s| {
                chrono::DateTime::parse_from_rfc3339(s)
                    .ok()
                    .map(|d| d.with_timezone(&chrono::Utc))
            }),
            source,
            reasoning: self.reasoning,
        })
    }
}

/// Summary of an activity segment for daily digest generation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SegmentSummaryRecord {
    pub segment_id: String,
    pub start_time: String,
    pub end_time: String,
    pub duration_secs: u64,
    pub dominant_category: String,
    pub regime_id: Option<String>,
    pub app_breakdown: String,
    pub content_activities_json: String,
    pub context_switch_count: u32,
    pub llm_summary: Option<String>,
}

/// Minimal segment detail for enriching vector search results.
#[derive(Debug, Clone)]
pub struct SegmentDetailRecord {
    pub segment_id: String,
    pub start_time: String,
    pub end_time: String,
    pub duration_secs: u64,
    pub llm_summary: Option<String>,
    pub dominant_category: String,
    pub regime_label: Option<String>,
}

/// Input DTO for inserting a GUI interaction event (V13, extended V22).
#[derive(Debug, Clone)]
pub struct NewGuiInteraction<'a> {
    pub event_id: &'a str,
    pub segment_id: Option<&'a str>,
    pub timestamp: &'a str,
    pub element_text: Option<&'a str>,
    pub element_type: Option<&'a str>,
    pub interaction_type: &'a str,
    pub bbox_json: Option<&'a str>,
    pub app_name: &'a str,
    /// Classification confidence for the inferred element type (0.0-1.0).
    /// Added in V22; defaults to 1.0 for backward compatibility.
    pub type_confidence: f32,
}

/// A GUI interaction event record from the V13 gui_interactions table (extended V22).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GuiInteractionRecord {
    pub id: i64,
    pub event_id: String,
    pub segment_id: Option<String>,
    pub timestamp: String,
    pub element_text: Option<String>,
    pub element_type: Option<String>,
    pub interaction_type: String,
    pub bbox_json: Option<String>,
    pub app_name: String,
    pub created_at: String,
    /// Classification confidence for the inferred element type (0.0-1.0).
    /// Added in V22; defaults to 1.0 for rows created before V22.
    #[serde(default = "default_type_confidence")]
    pub type_confidence: f32,
}

fn default_type_confidence() -> f32 {
    1.0
}

/// Row from the `feedback_retries` table (V24).
#[derive(Debug, Clone)]
pub struct PendingFeedbackRecord {
    pub id: Option<i64>,
    pub suggestion_id: String,
    pub feedback_type: String,
    pub comment: Option<String>,
    pub attempts: u32,
    pub next_retry_at: String,
    pub created_at: String,
}

/// Daily aggregated suggestion statistics for time-series display.
#[derive(Debug, Clone)]
pub struct DailyStatRecord {
    pub day: String,
    pub total: u32,
    pub acted: u32,
    pub suggestion_type: String,
    pub source: String,
}

impl PendingFeedbackRecord {
    /// Create a record for insertion (no id, auto-generated created_at).
    pub fn new_for_insert(
        suggestion_id: String,
        feedback_type: &crate::models::suggestion::FeedbackType,
        comment: Option<String>,
        attempts: u32,
        next_retry_at: chrono::DateTime<chrono::Utc>,
    ) -> Self {
        let ft = match feedback_type {
            crate::models::suggestion::FeedbackType::Accepted => "Accepted",
            crate::models::suggestion::FeedbackType::Rejected => "Rejected",
            crate::models::suggestion::FeedbackType::Deferred => "Deferred",
        };
        Self {
            id: None,
            suggestion_id,
            feedback_type: ft.to_string(),
            comment,
            attempts,
            next_retry_at: next_retry_at.to_rfc3339(),
            created_at: chrono::Utc::now().to_rfc3339(),
        }
    }

    /// Convert back to domain types. Returns `None` if feedback_type is unrecognized.
    #[allow(clippy::type_complexity)]
    pub fn into_domain_parts(
        self,
    ) -> Option<(
        String,
        crate::models::suggestion::FeedbackType,
        Option<String>,
        u32,
        chrono::DateTime<chrono::Utc>,
    )> {
        let ft = match self.feedback_type.as_str() {
            "Accepted" | "ACCEPTED" => crate::models::suggestion::FeedbackType::Accepted,
            "Rejected" | "REJECTED" => crate::models::suggestion::FeedbackType::Rejected,
            "Deferred" | "DEFERRED" => crate::models::suggestion::FeedbackType::Deferred,
            _ => return None,
        };
        let next_retry = chrono::DateTime::parse_from_rfc3339(&self.next_retry_at)
            .ok()
            .map(|d| d.with_timezone(&chrono::Utc))
            .unwrap_or_else(chrono::Utc::now);
        Some((
            self.suggestion_id,
            ft,
            self.comment,
            self.attempts,
            next_retry,
        ))
    }
}
