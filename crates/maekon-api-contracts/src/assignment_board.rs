//! Read-only consumer fields of the server's assignment-board.v1 DTOs (#12133).
//! Source: server/application/workflow_orchestration/dtos/assignment_board_read_model_dtos.py.
//! Presentation-only server fields are ignored. These DTOs are untrusted wire data,
//! have no Debug implementation, and confer no permission to write a cell.

use chrono::{DateTime, Utc};
use serde::Deserialize;

#[derive(Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssignmentBoardStateDto {
    Ready,
    Empty,
    Unavailable,
    Stale,
}

#[derive(Clone, Deserialize)]
pub struct AssignmentBoardSnapshotDto {
    pub contract_version: String,
    pub state: AssignmentBoardStateDto,
    pub organization_id: String,
    pub snapshot: Option<AssignmentBoardSnapshotIdentityDto>,
    pub approval: Option<AssignmentBoardApprovalDto>,
    pub synthetic: bool,
    #[serde(default)]
    pub provenance: Vec<String>,
    #[serde(default)]
    pub wbs_items: Vec<AssignmentBoardWbsItemDto>,
    #[serde(default)]
    pub candidates: Vec<AssignmentBoardCandidateDto>,
    pub page: AssignmentBoardPageDto,
}

#[derive(Clone, Deserialize)]
pub struct AssignmentBoardSnapshotIdentityDto {
    pub snapshot_id: String,
    pub snapshot_version: String,
    pub snapshot_hash: String,
    pub capacity_snapshot_id: String,
    pub capacity_snapshot_version: String,
    pub capacity_snapshot_hash: String,
    pub wbs_version_id: String,
    pub wbs_content_hash: String,
    pub capacity_wbs_binding_hash: String,
    pub approval_id: String,
}

#[derive(Clone, Deserialize)]
pub struct AssignmentBoardApprovalDto {
    pub approval_id: String,
    pub version_id: String,
    pub content_hash: String,
    pub approved_at: DateTime<Utc>,
    pub decision: String,
}

#[derive(Clone, Deserialize)]
pub struct AssignmentBoardWbsItemDto {
    pub item_id: String,
    pub skill_id: String,
}

#[derive(Clone, Deserialize)]
pub struct AssignmentBoardCandidateDto {
    pub candidate_id: String,
    pub candidate_hash: String,
    pub rank: u32,
    pub user_id: String,
    pub display_name: String,
    #[serde(default)]
    pub skill_ids: Vec<String>,
    pub required_skill_id: Option<String>,
    pub skill_match: Option<bool>,
    pub total_remaining_hours: f64,
    pub total_allocated_hours: f64,
    pub overlap_hours: f64,
    pub eligible: bool,
    pub rank_reason: String,
    pub algorithm_id: String,
    pub algorithm_version: String,
    pub algorithm_as_of: DateTime<Utc>,
    pub source_snapshot_id: String,
    pub source_snapshot_version: String,
    pub source_snapshot_hash: String,
    pub slots: Vec<AssignmentBoardSlotDto>,
}

#[derive(Clone, Deserialize)]
pub struct AssignmentBoardSlotDto {
    pub slot_id: String,
    pub capacity_hours: f64,
    pub allocated_hours: f64,
    pub remaining_hours: f64,
    pub evidence_status: AssignmentBoardStateDto,
}

#[derive(Clone, Deserialize)]
pub struct AssignmentBoardPageDto {
    pub limit: u8,
    pub total_candidates: u32,
    pub returned_candidates: u32,
    pub truncated: bool,
    pub next_cursor: Option<String>,
}
