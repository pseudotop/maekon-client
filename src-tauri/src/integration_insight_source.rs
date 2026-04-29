use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, NaiveDateTime, Utc};
use oneshim_core::error::CoreError;
use oneshim_core::models::integration::{
    InsightPacket, InsightSourceWindow, IntegrationCapabilityScope, IntegrationEnvelope,
    IntegrationInsightCandidate, IntegrationMessageType, IntegrationOrigin,
    IntegrationPrivacyClassification,
};
use oneshim_core::models::storage_records::LocalSuggestionRecord;
use oneshim_core::ports::integration::{IntegrationInsightSourcePort, LocalSuggestionQueryPort};

const LOCAL_SUGGESTION_NAMESPACE: &str = "focus.local_suggestions";

pub struct LocalSuggestionIntegrationSource {
    device_id: String,
    source_label: String,
    query: Arc<dyn LocalSuggestionQueryPort>,
}

impl LocalSuggestionIntegrationSource {
    pub fn new(device_id: impl Into<String>, query: Arc<dyn LocalSuggestionQueryPort>) -> Self {
        Self {
            device_id: device_id.into(),
            source_label: LOCAL_SUGGESTION_NAMESPACE.to_string(),
            query,
        }
    }

    fn parse_after_id(after_cursor: Option<String>) -> Result<Option<i64>, CoreError> {
        after_cursor
            .map(|cursor| {
                cursor.parse::<i64>().map_err(|err| CoreError::Validation {
                    code: oneshim_core::error_codes::ValidationCode::InvalidField,
                    field: "integration.checkpoint_cursor".to_string(),
                    message: format!("invalid local suggestion checkpoint: {err }"),
                })
            })
            .transpose()
    }

    fn parse_timestamp(raw: &str) -> Result<DateTime<Utc>, CoreError> {
        if let Ok(parsed) = DateTime::parse_from_rfc3339(raw) {
            return Ok(parsed.with_timezone(&Utc));
        }
        NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S")
            .map(|parsed| parsed.and_utc())
            .map_err(|err| CoreError::Validation {
                code: oneshim_core::error_codes::ValidationCode::InvalidField,
                field: "local_suggestions.created_at".to_string(),
                message: format!("invalid suggestion timestamp: {err }"),
            })
    }

    fn summarize(record: &LocalSuggestionRecord) -> (String, Vec<String>) {
        match record.suggestion_type.as_str() {
            "NeedFocusTime" => {
                let ratio = record
                    .payload
                    .get("communication_ratio")
                    .and_then(|value| value.as_f64())
                    .map(|ratio| (ratio * 100.0).round() as u32);
                let mins = record
                    .payload
                    .get("suggested_focus_mins")
                    .and_then(|value| value.as_u64());
                let summary = match (ratio, mins) {
                    (Some(ratio), Some(mins)) => format!(
                        "High communication load detected ({ratio}% of active time); {mins} minutes of protected focus time were recommended."
                    ),
                    _ => "High communication load detected; protected focus time was recommended."
                        .to_string(),
                };
                (summary, vec!["focus".to_string(), "communication_balance".to_string()])
            }
            "TakeBreak" => {
                let mins = record
                    .payload
                    .get("continuous_work_mins")
                    .and_then(|value| value.as_u64());
                let summary = mins.map_or_else(
                    || {
                        "Sustained concentration was detected and a short break was recommended."
                            .to_string()
                    },
                    |mins| {
                        format!(
                            "Sustained concentration was detected after {mins} minutes of deep work; a short break was recommended."
                        )
                    },
                );
                (summary, vec!["focus".to_string(), "break".to_string()])
            }
            "RestoreContext" => (
                "A recent interruption was detected and a context restoration prompt was generated."
                    .to_string(),
                vec!["focus".to_string(), "context_restore".to_string()],
            ),
            "PatternDetected" => {
                let confidence = record
                    .payload
                    .get("confidence")
                    .and_then(|value| value.as_f64())
                    .map(|confidence| (confidence * 100.0).round() as u32);
                let summary = confidence.map_or_else(
                    || "A recurring workflow pattern was detected locally.".to_string(),
                    |confidence| {
                        format!(
                            "A recurring workflow pattern was detected locally (confidence {confidence}%)."
                        )
                    },
                );
                (summary, vec!["workflow".to_string(), "pattern".to_string()])
            }
            "ExcessiveCommunication" => {
                let today = record
                    .payload
                    .get("today_communication_mins")
                    .and_then(|value| value.as_u64());
                let avg = record
                    .payload
                    .get("avg_communication_mins")
                    .and_then(|value| value.as_u64());
                let summary = match (today, avg) {
                    (Some(today), Some(avg)) => format!(
                        "Communication time exceeded the recent baseline ({today} minutes today vs {avg} minutes average)."
                    ),
                    _ => "Communication time exceeded the recent baseline.".to_string(),
                };
                (summary, vec!["focus".to_string(), "communication_balance".to_string()])
            }
            _ => (
                "A locally derived productivity insight is ready for integration delivery."
                    .to_string(),
                vec!["focus".to_string()],
            ),
        }
    }

    fn to_candidate(
        &self,
        record: LocalSuggestionRecord,
    ) -> Result<IntegrationInsightCandidate, CoreError> {
        let occurred_at = Self::parse_timestamp(&record.created_at)?;
        let (summary, derived_tags) = Self::summarize(&record);
        let cursor = record.id.to_string();

        Ok(IntegrationInsightCandidate {
            source_cursor: cursor.clone(),
            envelope: IntegrationEnvelope {
                envelope_id: format!("integration.focus.local_suggestion.{cursor}"),
                schema_version: "integration.envelope.v1".to_string(),
                message_type: IntegrationMessageType::InsightPacket,
                timestamp: occurred_at,
                nonce: format!("focus-local-suggestion-{cursor}"),
                origin: IntegrationOrigin {
                    device_id: self.device_id.clone(),
                    workspace_id: None,
                    session_id: None,
                    source: self.source_label.clone(),
                },
                capability_scope: IntegrationCapabilityScope::InsightWrite,
            },
            packet: InsightPacket {
                packet_id: format!("local_suggestion:{cursor}"),
                summary,
                derived_tags,
                source_window: InsightSourceWindow {
                    started_at: occurred_at,
                    ended_at: occurred_at,
                },
                privacy_classification: IntegrationPrivacyClassification::DerivedSummary,
                audit_reference_id: Some(format!("local_suggestion_record:{cursor}")),
            },
        })
    }
}

#[async_trait]
impl IntegrationInsightSourcePort for LocalSuggestionIntegrationSource {
    fn checkpoint_namespace(&self) -> &'static str {
        LOCAL_SUGGESTION_NAMESPACE
    }

    async fn list_candidates_after(
        &self,
        after_cursor: Option<String>,
        limit: usize,
    ) -> Result<Vec<IntegrationInsightCandidate>, CoreError> {
        let after_id = Self::parse_after_id(after_cursor)?;
        self.query
            .list_local_suggestions_after(after_id, limit.max(1))
            .await?
            .into_iter()
            .map(|record| self.to_candidate(record))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use oneshim_core::models::storage_records::LocalSuggestionRecord;
    use tokio::sync::Mutex;

    use super::*;

    struct MockSuggestionQuery {
        records: Arc<Mutex<Vec<LocalSuggestionRecord>>>,
        seen_after: Arc<Mutex<Option<i64>>>,
    }

    #[async_trait]
    impl LocalSuggestionQueryPort for MockSuggestionQuery {
        async fn list_local_suggestions_after(
            &self,
            after_id: Option<i64>,
            _limit: usize,
        ) -> Result<Vec<LocalSuggestionRecord>, CoreError> {
            *self.seen_after.lock().await = after_id;
            Ok(self.records.lock().await.clone())
        }
    }

    #[tokio::test]
    async fn local_suggestion_source_sanitizes_restore_context_payload() {
        let query = Arc::new(MockSuggestionQuery {
            records: Arc::new(Mutex::new(vec![LocalSuggestionRecord {
                id: 42,
                suggestion_type: "RestoreContext".to_string(),
                payload: serde_json::json!({
                    "interrupted_app": "Code",
                    "interrupted_at": "2026-03-16T08:00:00Z",
                    "snapshot_frame_id": 123
                }),
                created_at: "2026-03-16T08:00:00Z".to_string(),
                shown_at: None,
                dismissed_at: None,
                acted_at: None,
            }])),
            seen_after: Arc::new(Mutex::new(None)),
        });
        let source = LocalSuggestionIntegrationSource::new("device-1", query.clone());

        let candidates = source
            .list_candidates_after(Some("41".to_string()), 10)
            .await
            .unwrap();

        assert_eq!(*query.seen_after.lock().await, Some(41));
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].packet.packet_id, "local_suggestion:42");
        assert!(candidates[0].packet.summary.contains("context restoration"));
        assert!(!candidates[0].packet.summary.contains("Code"));
    }
}
