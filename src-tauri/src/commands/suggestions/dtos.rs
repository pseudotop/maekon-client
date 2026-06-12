//! Response and event DTOs for suggestion Tauri commands.
//!
//! ADR-013 split from `suggestions/mod.rs`.

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

#[derive(Serialize)]
pub struct SuggestionViewDto {
    pub id: String,
    pub title: String,
    pub body: String,
    pub priority: String,
    pub category: Option<String>,
    pub source: String,
    pub confidence_score: f64,
    pub created_at: String,
    pub is_read: bool,
    pub reasoning: Option<String>,
    pub context_scope: Option<SuggestionContextScopeDto>,
}

#[derive(Clone, Serialize)]
pub struct SuggestionContextScopeDto {
    pub app_name: Option<String>,
    pub window_title: Option<String>,
    pub target_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SuggestionReplayEventPayload {
    pub event_name: String,
    pub phase: String,
    pub suggestion_id: Option<String>,
    pub target_id: Option<String>,
    pub surface_placement: String,
    pub app_name: Option<String>,
    pub window_title: Option<String>,
    pub action: Option<String>,
    pub audit_ready: bool,
    pub raw_context_included: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SuggestionReplayEventAck {
    pub trace_id: String,
    pub recorded: bool,
}

#[derive(Serialize)]
pub struct SuggestionHistoryDto {
    pub suggestion: SuggestionViewDto,
    pub feedback: Option<String>,
}

#[derive(Serialize)]
pub struct TypeCountDto {
    pub suggestion_type: String,
    pub count: u32,
}

#[derive(Serialize)]
pub struct SourceStatsDto {
    pub source: String,
    pub count: u32,
    pub accepted: u32,
    pub rejected: u32,
}

#[derive(Serialize)]
pub struct SuggestionStatsDto {
    pub total_shown: u32,
    pub total_accepted: u32,
    pub total_rejected: u32,
    pub total_deferred: u32,
    pub acceptance_rate: f64,
    pub by_type: Vec<TypeCountDto>,
    pub by_source: Vec<SourceStatsDto>,
}

#[derive(Serialize)]
pub struct DailyStatDto {
    pub date: String,
    pub shown: u32,
    pub accepted: u32,
    pub rejected: u32,
    pub deferred: u32,
}

#[derive(Serialize)]
pub struct DeferredSuggestionDto {
    pub id: String,
    pub title: String,
    pub body: String,
    pub priority: String,
    pub source: String,
    pub deferred_at: String,
    pub resurface_at: String,
    pub remaining_minutes: i64,
}
