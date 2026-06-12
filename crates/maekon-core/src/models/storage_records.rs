/// Row types returned by the web storage port.
///
/// These structs model rows retrieved from SQLite queries. They live in
/// `maekon-core` so that the `WebStorage` port trait (also in core) can
/// reference them without pulling in the `maekon-storage` adapter crate.

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
    pub context_app: Option<String>,
    pub context_window: Option<String>,
    pub context_target_id: Option<String>,
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
            "BREAK_REMINDER" | "BreakReminder" => SuggestionType::BreakReminder,
            "FOCUS_MODE" | "FocusMode" => SuggestionType::FocusMode,
            "TAKE_BREAK" | "TakeBreak" => SuggestionType::TakeBreak,
            "NEED_FOCUS_TIME" | "NeedFocusTime" => SuggestionType::NeedFocusTime,
            "RESTORE_CONTEXT" | "RestoreContext" => SuggestionType::RestoreContext,
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
        let context_scope = suggestion_context_scope_from_record(
            self.context_app,
            self.context_window,
            self.context_target_id,
        );
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
            context_scope,
        })
    }
}

fn suggestion_context_scope_from_record(
    context_app: Option<String>,
    context_window: Option<String>,
    context_target_id: Option<String>,
) -> Option<crate::models::suggestion::SuggestionContextScope> {
    use crate::models::suggestion::SuggestionContextScope;

    let scope = SuggestionContextScope {
        app_name: non_empty_context_value(context_app),
        window_title: non_empty_context_value(context_window),
        target_id: non_empty_context_value(context_target_id),
    };

    if scope.app_name.is_none() && scope.window_title.is_none() && scope.target_id.is_none() {
        None
    } else {
        Some(scope)
    }
}

fn non_empty_context_value(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

/// Egress 감사 원장 한 행 (`egress_ledger` 테이블, V36, #4803/E20).
///
/// 디바이스를 떠난(`disposition='uploaded'`) 또는 정책상 차단된
/// (`disposition='blocked'`) 이벤트를 규제 준수 증거로 기록한다.
/// `IntegrationInsightAuditRecord` 와 동일한 형태(serde + Clone + Debug)를 따른다.
fn default_recipient_count() -> i64 {
    1
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EgressLedgerRecord {
    /// 호출자가 생성하는 UUID. `egress_ledger.record_id` UNIQUE 로 재실행 중복 제거.
    pub record_id: String,
    /// 이벤트 유형. 텔레메트리 producer: Context/Window/User/Input/Process/System/
    /// Clipboard/FileAccess. 동기화 producer(#5143): `CrossDeviceSync`(일반 push)
    /// 또는 `DeletionEvent`(GDPR Art.17 tombstone push).
    pub event_type: String,
    /// 연관 이벤트 id (nullable). events 테이블 id 또는 생성된 식별자.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_id: Option<String>,
    /// 직렬화된 payload 바이트 크기. 평문(plaintext) 직렬화 크기이며, 암호화/압축
    /// 후의 on-wire 크기가 아니다(#5143). LAN fan-out 은 동일 직렬화를 N 피어로
    /// 보내므로 실제 egress 총량 = `byte_count * recipient_count` (#5147 item 2).
    pub byte_count: i64,
    /// egress 수신처 개수(#5147 item 2). LAN multi-peer push = 전달 성공 피어 수,
    /// File/Remote(단일 목적지) = 1. 누락 시(이전 레코드/텔레메트리) 1 로 기본.
    #[serde(default = "default_recipient_count")]
    pub recipient_count: i64,
    /// egress 대상 (업로드 엔드포인트 / sink 타깃 문자열). 텔레메트리:
    /// `server.batch_upload`. 동기화: `sync.lan`/`sync.remote`/`sync.file`
    /// (peer/endpoint 상세는 일부러 기록하지 않음).
    pub destination: String,
    /// egress 처분 — `'uploaded'` 또는 `'blocked'`.
    pub disposition: String,
    /// egress 시점의 동의 스냅샷. 텔레메트리 path = telemetry/upload 동의; 동기화
    /// path = `cross_device_sync=<bool>`(#5143).
    pub consent_state: String,
    /// 발생 시각 (RFC3339).
    pub occurred_at: String,
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
