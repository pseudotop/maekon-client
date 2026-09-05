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
    /// #8045 B1: derived-data cascade counts. A range delete now also removes
    /// the LLM summaries (`activity_segments`), embeddings (`embedding_vectors`),
    /// and local suggestions overlapping the window that are derived from the
    /// deleted events/frames — otherwise sensitive content survived in the
    /// derived tables. Not part of the frozen `DeleteResult` HTTP contract; used
    /// for the deletion-summary message and audit/log evidence only.
    pub activity_segments_deleted: u64,
    pub embedding_vectors_deleted: u64,
    pub local_suggestions_deleted: u64,
    /// #8059: voice transcripts (V47) removed for the window. A range delete
    /// that clears events/frames content also removes the speech transcripts
    /// recorded in that period (voice-activity content), keeping the "meetings
    /// stay on your machine" data under the same erasure discipline. Local-only
    /// table — no sync tombstone. Like the derived-data counts above, this is
    /// audit/message-only and NOT part of the frozen `DeleteResult` HTTP contract.
    pub transcripts_deleted: u64,
}

impl DeletedRangeCounts {
    /// Total rows removed across primary + derived tables. Used to build the
    /// user-facing "N records were deleted" message so the count reflects the
    /// derived-data cascade (#8045 B1) and the voice-transcript cascade (#8059).
    pub fn total(&self) -> u64 {
        self.events_deleted
            + self.frames_deleted
            + self.metrics_deleted
            + self.process_snapshots_deleted
            + self.idle_periods_deleted
            + self.activity_segments_deleted
            + self.embedding_vectors_deleted
            + self.local_suggestions_deleted
            + self.transcripts_deleted
    }
}

/// Row counts removed by a retroactive per-app deletion (#8045 B2 — Recall
/// parity). When an app (or app pattern) is newly added to the exclusion list,
/// its already-recorded history is purged: frame metadata + files, events, and
/// the derived LLM summaries (`activity_segments`) + embeddings
/// (`embedding_vectors`) attributable to that app.
///
/// `local_suggestions` is deliberately NOT included: it carries no reliable
/// per-app identifier (the app context lives in a separate feedback table and
/// the `payload` holds signal metrics, not app names), so a substring match
/// would over-delete unrelated suggestions. It is a short-lived, regenerated
/// cache covered by the time-range and full-wipe deletion primitives instead.
#[derive(Debug, Clone, Default)]
pub struct AppDeletionCounts {
    pub events_deleted: u64,
    pub frames_deleted: u64,
    pub activity_segments_deleted: u64,
    pub embedding_vectors_deleted: u64,
}

impl AppDeletionCounts {
    /// Total rows removed across the matched tables (excludes on-disk frame
    /// files, which the caller deletes best-effort from the returned paths).
    pub fn total(&self) -> u64 {
        self.events_deleted
            + self.frames_deleted
            + self.activity_segments_deleted
            + self.embedding_vectors_deleted
    }
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
        // Parse `expires_at` strictly, mirroring `created_at` below. A NULL
        // `expires_at` legitimately means "never expires", but a present
        // value that fails RFC3339 parsing is a corrupt record: silently
        // mapping it to `None` would flip a (possibly already-expired)
        // suggestion into one that never expires. Drop such records instead,
        // and emit a warning so the drift is observable.
        let expires_at = match self.expires_at.as_deref() {
            None => None,
            Some(raw) => match chrono::DateTime::parse_from_rfc3339(raw) {
                Ok(parsed) => Some(parsed.with_timezone(&chrono::Utc)),
                Err(_) => {
                    tracing::warn!(
                        suggestion_id = %self.suggestion_id,
                        raw,
                        "dropping suggestion: unparseable expires_at"
                    );
                    return None;
                }
            },
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
            expires_at,
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

/// A single row of the egress audit ledger (`egress_ledger` table, V36, #4803/E20).
///
/// Records, as regulatory-compliance evidence, events that either left the
/// device (`disposition='uploaded'`) or were blocked by policy
/// (`disposition='blocked'`). Follows the same shape (serde + Clone + Debug)
/// as `IntegrationInsightAuditRecord`.
fn default_recipient_count() -> i64 {
    1
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EgressLedgerRecord {
    /// Caller-generated UUID. `egress_ledger.record_id` is UNIQUE, so re-runs
    /// are deduplicated.
    pub record_id: String,
    /// Event type. Telemetry producers: Context/Window/User/Input/Process/System/
    /// Clipboard/FileAccess. Sync producers (#5143): `CrossDeviceSync` (normal
    /// push) or `DeletionEvent` (GDPR Art.17 tombstone push).
    pub event_type: String,
    /// Associated event id (nullable). The events-table id or a generated identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_id: Option<String>,
    /// Serialized payload byte size. This is the plaintext serialized size, not
    /// the on-wire size after encryption/compression (#5143). LAN fan-out sends
    /// the same serialization to N peers, so the actual total egress =
    /// `byte_count * recipient_count` (#5147 item 2).
    pub byte_count: i64,
    /// Number of egress recipients (#5147 item 2). LAN multi-peer push = number
    /// of peers delivered to successfully; File/Remote (single destination) = 1.
    /// Defaults to 1 when absent (older records / telemetry).
    #[serde(default = "default_recipient_count")]
    pub recipient_count: i64,
    /// Egress destination (upload endpoint / sink target string). Telemetry:
    /// `server.batch_upload`. Sync: `sync.lan`/`sync.remote`/`sync.file`
    /// (peer/endpoint details are deliberately not recorded).
    pub destination: String,
    /// Egress disposition — `'uploaded'` or `'blocked'`.
    pub disposition: String,
    /// Consent snapshot at the egress moment. Telemetry path = telemetry/upload
    /// consent; sync path = `cross_device_sync=<bool>` (#5143).
    pub consent_state: String,
    /// Occurrence timestamp (RFC3339).
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
    #[serde(default)]
    pub ai_summary: crate::models::ai_summary::AiSummaryArtifact,
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
///
/// #7678 D3: `segment_id`/`element_text`/`element_type`/`bbox_json` were
/// removed (V43) — the production writer never populated them (constant
/// `element_type: Some("Click")` and `segment_id`/`element_text`/`bbox_json`
/// always `None`), so the columns advertised richer data than the table ever
/// carried. See `migration/v43_gui_interactions_drop_unused_columns.rs`.
#[derive(Debug, Clone)]
pub struct NewGuiInteraction<'a> {
    pub event_id: &'a str,
    pub timestamp: &'a str,
    pub interaction_type: &'a str,
    pub app_name: &'a str,
    /// Classification confidence for the inferred element type (0.0-1.0).
    /// Added in V22; defaults to 1.0 for backward compatibility.
    pub type_confidence: f32,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::suggestion::SuggestionSource;

    /// Build a minimal valid `SuggestionRecord` with the given `expires_at`.
    fn record_with_expires_at(expires_at: Option<&str>) -> SuggestionRecord {
        SuggestionRecord {
            id: 1,
            suggestion_id: "sug-1".to_string(),
            suggestion_type: "WORK_GUIDANCE".to_string(),
            source: SuggestionSource::RULE_BASED_STR.to_string(),
            content: "do the thing".to_string(),
            priority: "MEDIUM".to_string(),
            confidence_score: 0.9,
            relevance_score: 0.8,
            is_actionable: true,
            reasoning: None,
            context_app: None,
            context_window: None,
            context_target_id: None,
            shown_at: None,
            dismissed_at: None,
            acted_at: None,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            expires_at: expires_at.map(|s| s.to_string()),
            resurface_at: None,
        }
    }

    #[test]
    fn absent_expires_at_means_never_expires() {
        let suggestion = record_with_expires_at(None)
            .try_into_suggestion()
            .expect("record with no expires_at should convert");
        assert!(suggestion.expires_at.is_none());
    }

    #[test]
    fn valid_expires_at_is_parsed() {
        let suggestion = record_with_expires_at(Some("2026-02-01T12:30:00Z"))
            .try_into_suggestion()
            .expect("record with valid expires_at should convert");
        let expected = chrono::DateTime::parse_from_rfc3339("2026-02-01T12:30:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        assert_eq!(suggestion.expires_at, Some(expected));
    }

    #[test]
    fn malformed_expires_at_drops_record() {
        // A present-but-unparseable expires_at must NOT be silently treated as
        // "never expires"; the corrupt record is dropped (mirrors created_at).
        assert!(record_with_expires_at(Some("not-a-timestamp"))
            .try_into_suggestion()
            .is_none());
    }

    #[test]
    fn malformed_created_at_drops_record() {
        // Sanity check that the existing strict created_at handling is unchanged.
        let mut record = record_with_expires_at(None);
        record.created_at = "not-a-timestamp".to_string();
        assert!(record.try_into_suggestion().is_none());
    }

    /// #10197 Wave 2: mutation guards. 48 mutants survived here — the two
    /// deletion `total()` sums, the record->domain match arms (the SQLite read
    /// path: a deleted arm silently drops stored suggestions/feedback on
    /// restart), the context-scope conjunction, and the egress-ledger
    /// recipient-count default. Nested in `tests` to reuse
    /// `record_with_expires_at`.
    mod mutation_guard {
        use super::*;
        use crate::models::suggestion::{FeedbackType, Priority, SuggestionType};

        /// Powers of two make every term independently visible: any single
        /// `+` -> `-`/`*` changes the total, and no two subsets collide.
        #[test]
        fn deleted_range_total_counts_every_table_exactly_once() {
            let counts = DeletedRangeCounts {
                events_deleted: 1,
                frames_deleted: 2,
                metrics_deleted: 4,
                process_snapshots_deleted: 8,
                idle_periods_deleted: 16,
                activity_segments_deleted: 32,
                embedding_vectors_deleted: 64,
                local_suggestions_deleted: 128,
                transcripts_deleted: 256,
            };
            assert_eq!(
                counts.total(),
                511,
                "every table term must add exactly once"
            );
        }

        #[test]
        fn app_deletion_total_counts_every_table_exactly_once() {
            let counts = AppDeletionCounts {
                events_deleted: 1,
                frames_deleted: 2,
                activity_segments_deleted: 4,
                embedding_vectors_deleted: 8,
            };
            assert_eq!(counts.total(), 15);
        }

        /// Both stored spellings of every suggestion-type arm must parse.
        /// These strings are what SQLite rows actually contain; a deleted arm
        /// makes every stored suggestion of that type vanish on read.
        #[test]
        fn suggestion_type_arms_parse_both_stored_spellings() {
            let cases: &[(&str, &str, SuggestionType)] = &[
                (
                    "WORK_GUIDANCE",
                    "WorkGuidance",
                    SuggestionType::WorkGuidance,
                ),
                ("EMAIL_DRAFT", "EmailDraft", SuggestionType::EmailDraft),
                (
                    "PRODUCTIVITY_TIP",
                    "ProductivityTip",
                    SuggestionType::ProductivityTip,
                ),
                (
                    "WORKFLOW_OPTIMIZATION",
                    "WorkflowOptimization",
                    SuggestionType::WorkflowOptimization,
                ),
                (
                    "CONTEXT_BASED",
                    "ContextBased",
                    SuggestionType::ContextBased,
                ),
                (
                    "BREAK_REMINDER",
                    "BreakReminder",
                    SuggestionType::BreakReminder,
                ),
                ("FOCUS_MODE", "FocusMode", SuggestionType::FocusMode),
                ("TAKE_BREAK", "TakeBreak", SuggestionType::TakeBreak),
                (
                    "NEED_FOCUS_TIME",
                    "NeedFocusTime",
                    SuggestionType::NeedFocusTime,
                ),
                (
                    "RESTORE_CONTEXT",
                    "RestoreContext",
                    SuggestionType::RestoreContext,
                ),
            ];
            for (screaming, pascal, expected) in cases {
                for spelling in [screaming, pascal] {
                    let mut record = record_with_expires_at(None);
                    record.suggestion_type = (*spelling).to_string();
                    let suggestion = record
                        .try_into_suggestion()
                        .unwrap_or_else(|| panic!("{spelling} must parse"));
                    assert_eq!(suggestion.suggestion_type, *expected, "{spelling}");
                }
            }
            let mut record = record_with_expires_at(None);
            record.suggestion_type = "NO_SUCH_TYPE".to_string();
            assert!(
                record.try_into_suggestion().is_none(),
                "unknown type is a None, not a default"
            );
        }

        #[test]
        fn priority_and_source_arms_parse_both_stored_spellings() {
            use crate::models::suggestion::SuggestionSource;
            let priorities: &[(&str, &str, Priority)] = &[
                ("LOW", "Low", Priority::Low),
                ("HIGH", "High", Priority::High),
                ("CRITICAL", "Critical", Priority::Critical),
            ];
            for (screaming, pascal, expected) in priorities {
                for spelling in [screaming, pascal] {
                    let mut record = record_with_expires_at(None);
                    record.priority = (*spelling).to_string();
                    let suggestion = record.try_into_suggestion().expect("valid record");
                    assert_eq!(suggestion.priority, *expected, "{spelling}");
                }
            }
            // Unknown priority deliberately falls back to Medium (not None).
            let mut record = record_with_expires_at(None);
            record.priority = "NO_SUCH_PRIORITY".to_string();
            assert_eq!(
                record.try_into_suggestion().expect("valid record").priority,
                Priority::Medium
            );

            let sources: &[(&str, SuggestionSource)] = &[
                (
                    SuggestionSource::LLM_SERVER_STR,
                    SuggestionSource::LlmServer,
                ),
                ("LlmServer", SuggestionSource::LlmServer),
                (SuggestionSource::LLM_LOCAL_STR, SuggestionSource::LlmLocal),
                ("LlmLocal", SuggestionSource::LlmLocal),
            ];
            for (spelling, expected) in sources {
                let mut record = record_with_expires_at(None);
                record.source = (*spelling).to_string();
                let suggestion = record.try_into_suggestion().expect("valid record");
                assert_eq!(suggestion.source, *expected, "{spelling}");
            }
        }

        /// The scope vanishes only when ALL THREE axes are empty — each `&&`
        /// arm alone must keep a one-axis scope alive. `&&` -> `||` would drop
        /// a scope the moment ANY axis is empty, silently widening every
        /// context-scoped suggestion to global.
        #[test]
        fn context_scope_survives_on_any_single_axis() {
            let one_axis = [
                (Some("app".to_string()), None, None),
                (None, Some("window".to_string()), None),
                (None, None, Some("target".to_string())),
            ];
            for (app, window, target) in one_axis {
                let scope = suggestion_context_scope_from_record(
                    app.clone(),
                    window.clone(),
                    target.clone(),
                );
                assert!(
                    scope.is_some(),
                    "a single populated axis must keep the scope: {app:?}/{window:?}/{target:?}"
                );
            }
            assert!(
                suggestion_context_scope_from_record(None, None, None).is_none(),
                "all-empty must be None, not an empty scope object"
            );
        }

        #[test]
        fn feedback_arms_parse_both_stored_spellings() {
            let cases: &[(&str, FeedbackType)] = &[
                ("Accepted", FeedbackType::Accepted),
                ("ACCEPTED", FeedbackType::Accepted),
                ("Rejected", FeedbackType::Rejected),
                ("REJECTED", FeedbackType::Rejected),
                ("Deferred", FeedbackType::Deferred),
                ("DEFERRED", FeedbackType::Deferred),
            ];
            for (spelling, expected) in cases {
                let record = PendingFeedbackRecord {
                    id: Some(1),
                    suggestion_id: "sug-9".to_string(),
                    feedback_type: (*spelling).to_string(),
                    comment: Some("note".to_string()),
                    attempts: 3,
                    next_retry_at: "2026-01-02T03:04:05Z".to_string(),
                    created_at: "2026-01-01T00:00:00Z".to_string(),
                };
                let (suggestion_id, ft, comment, attempts, next_retry) =
                    record.into_domain_parts().expect("known feedback type");
                assert_eq!(ft, *expected, "{spelling}");
                // Field pass-through — a `-> None` replacement or a swapped
                // tuple slot fails one of these.
                assert_eq!(suggestion_id, "sug-9");
                assert_eq!(comment.as_deref(), Some("note"));
                assert_eq!(attempts, 3);
                assert_eq!(next_retry.to_rfc3339(), "2026-01-02T03:04:05+00:00");
            }

            let unknown = PendingFeedbackRecord {
                id: None,
                suggestion_id: "sug-9".to_string(),
                feedback_type: "Shrugged".to_string(),
                comment: None,
                attempts: 0,
                next_retry_at: "2026-01-02T03:04:05Z".to_string(),
                created_at: "2026-01-01T00:00:00Z".to_string(),
            };
            assert!(unknown.into_domain_parts().is_none());
        }

        /// Egress-ledger rows predating V40 count as ONE recipient — 0 would
        /// erase their audited volume (`bytes * recipients`), -1 would negate it.
        #[test]
        fn recipient_count_default_is_exactly_one() {
            assert_eq!(default_recipient_count(), 1);
        }
    }
}
