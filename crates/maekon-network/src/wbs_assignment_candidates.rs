//! Authenticated read-only WBS candidates and validation of untrusted server data (#12133).
//! Candidate observations grant no authority to apply a cell change.

use crate::http_client::HttpApiClient;
use async_trait::async_trait;
use maekon_api_contracts::assignment_board::{
    AssignmentBoardCandidateDto, AssignmentBoardSnapshotDto, AssignmentBoardStateDto,
};
use maekon_core::models::wbs_assignment_candidates::{
    WbsAssignmentCandidate, WbsAssignmentCandidates, WbsCandidateAlgorithm, WbsCandidateError,
    WbsCandidateIdentity, WbsCandidateProvenance, WbsCandidateQuery, WbsCandidateSnapshot,
    WbsCandidateSnapshotRef,
};
use maekon_core::ports::wbs_assignment_candidates::WbsAssignmentCandidatesPort;
use maekon_http_core::outbound::{read_body_capped, BodyReadError};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};

/// Validate board availability, organization and approved composite source pins.
/// Task membership and expected WBS version are checked when assembling the result.
pub fn validate_board_source(
    board: &AssignmentBoardSnapshotDto,
    query: &WbsCandidateQuery,
) -> Result<WbsCandidateSnapshot, WbsCandidateError> {
    use AssignmentBoardStateDto as State;
    if board.contract_version
        != maekon_core::models::wbs_assignment_candidates::WBS_ASSIGNMENT_CONTRACT_VERSION
    {
        return Err(WbsCandidateError::UnsupportedContract);
    }
    if Some(board.organization_id.as_str()) != query.context().organization_id() {
        return Err(WbsCandidateError::Stale);
    }
    match board.state {
        State::Ready => {}
        State::Empty => return Err(WbsCandidateError::NoCandidates),
        State::Stale => return Err(WbsCandidateError::Stale),
        State::Unavailable => return Err(WbsCandidateError::Unavailable),
    }
    let snapshot = board
        .snapshot
        .as_ref()
        .ok_or(WbsCandidateError::InvalidResponse)?;
    let approval = board
        .approval
        .as_ref()
        .ok_or(WbsCandidateError::InvalidResponse)?;
    if approval.decision != "approved"
        || approval.approval_id != snapshot.approval_id
        || approval.version_id != snapshot.wbs_version_id
        || approval.content_hash != snapshot.wbs_content_hash
    {
        return Err(WbsCandidateError::Stale);
    }
    // Validate the additional composite source pins even though the portable
    // result only exposes the board and approved WBS identities.
    WbsCandidateSnapshotRef::new(
        snapshot.capacity_snapshot_id.clone(),
        snapshot.capacity_snapshot_version.clone(),
        snapshot.capacity_snapshot_hash.clone(),
    )?;
    if !valid_hash(&snapshot.capacity_wbs_binding_hash) {
        return Err(WbsCandidateError::InvalidResponse);
    }
    let reference = WbsCandidateSnapshotRef::new(
        snapshot.snapshot_id.clone(),
        snapshot.snapshot_version.clone(),
        snapshot.snapshot_hash.clone(),
    )?;
    WbsCandidateSnapshot::new(
        reference,
        snapshot.wbs_version_id.clone(),
        snapshot.wbs_content_hash.clone(),
        snapshot.approval_id.clone(),
    )
}

// Server v1 hashes a sorted compact object including the exact requested task.
/// Verify the server hash binds this candidate to the exact organization and task.
pub fn validate_candidate_binding(
    candidate: &AssignmentBoardCandidateDto,
    organization_id: &str,
    item_id: &str,
) -> Result<(), WbsCandidateError> {
    let required_skill = candidate
        .required_skill_id
        .as_deref()
        .ok_or(WbsCandidateError::InvalidResponse)?;
    let pins = BTreeMap::from([
        ("algorithm_id", candidate.algorithm_id.as_str()),
        ("algorithm_version", candidate.algorithm_version.as_str()),
        ("organization_id", organization_id),
        ("snapshot_id", candidate.source_snapshot_id.as_str()),
        (
            "snapshot_version",
            candidate.source_snapshot_version.as_str(),
        ),
        ("snapshot_hash", candidate.source_snapshot_hash.as_str()),
        ("wbs_item_id", item_id),
        ("required_skill_id", required_skill),
        ("user_id", candidate.user_id.as_str()),
    ]);
    // The server's identifiers are ASCII. This also makes Python's default
    // ensure_ascii and serde_json's UTF-8 string encoding identical.
    if pins.values().any(|id| !valid_id(id)) {
        return Err(WbsCandidateError::InvalidResponse);
    }
    let bytes = serde_json::to_vec(&pins).map_err(|_| WbsCandidateError::InvalidResponse)?;
    let hash = hex::encode(Sha256::digest(bytes));
    if candidate.candidate_hash != hash
        || candidate.candidate_id != format!("wd-candidate-{}", &hash[..24])
    {
        return Err(WbsCandidateError::Stale);
    }
    Ok(())
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
}

fn valid_hash(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
}

// Server v1 publishes ten business-day slots.
const BUSINESS_DAY_COUNT: usize = 10;
const CAPACITY_TOLERANCE: f64 = 1e-9;

/// Project one candidate after validating its skill and capacity evidence.
/// Callers must independently verify the board/task binding before use.
pub fn project_candidate(
    candidate: AssignmentBoardCandidateDto,
    required_skill: &str,
) -> Result<WbsAssignmentCandidate, WbsCandidateError> {
    let name = candidate.display_name.trim_start();
    if name.starts_with(['=', '+', '-', '@']) {
        return Err(WbsCandidateError::InvalidResponse);
    }
    if candidate.required_skill_id.as_deref() != Some(required_skill)
        || candidate.skill_match
            != Some(
                candidate
                    .skill_ids
                    .iter()
                    .any(|skill| skill == required_skill),
            )
        || candidate.eligible
            != (candidate.skill_match == Some(true)
                && candidate.total_remaining_hours > CAPACITY_TOLERANCE)
    {
        return Err(WbsCandidateError::InvalidResponse);
    }
    validate_capacity(&candidate)?;
    WbsAssignmentCandidate::new(
        WbsCandidateIdentity::new(
            candidate.candidate_id,
            candidate.candidate_hash,
            candidate.user_id,
        )?,
        candidate.display_name,
        candidate.rank,
        candidate.eligible,
        candidate.rank_reason,
        WbsCandidateSnapshotRef::new(
            candidate.source_snapshot_id,
            candidate.source_snapshot_version,
            candidate.source_snapshot_hash,
        )?,
        WbsCandidateAlgorithm::new(
            candidate.algorithm_id,
            candidate.algorithm_version,
            candidate.algorithm_as_of,
        )?,
    )
}

fn validate_capacity(candidate: &AssignmentBoardCandidateDto) -> Result<(), WbsCandidateError> {
    if candidate.slots.len() != BUSINESS_DAY_COUNT {
        return Err(WbsCandidateError::InvalidResponse);
    }
    let mut ids = HashSet::new();
    for slot in &candidate.slots {
        if !valid_id(&slot.slot_id)
            || !ids.insert(&slot.slot_id)
            || slot.evidence_status != AssignmentBoardStateDto::Ready
            || ![
                slot.capacity_hours,
                slot.allocated_hours,
                slot.remaining_hours,
            ]
            .into_iter()
            .all(|hours| hours.is_finite() && hours >= 0.0)
            || !equal_hours(
                slot.capacity_hours,
                slot.allocated_hours + slot.remaining_hours,
            )
        {
            return Err(WbsCandidateError::InvalidResponse);
        }
    }
    let remaining = candidate
        .slots
        .iter()
        .map(|slot| slot.remaining_hours)
        .sum();
    let allocated = candidate
        .slots
        .iter()
        .map(|slot| slot.allocated_hours)
        .sum();
    if !equal_hours(candidate.total_remaining_hours, remaining)
        || !equal_hours(candidate.total_allocated_hours, allocated)
        || !equal_hours(candidate.overlap_hours, allocated)
    {
        return Err(WbsCandidateError::InvalidResponse);
    }
    Ok(())
}

fn equal_hours(actual: f64, expected: f64) -> bool {
    actual.is_finite() && (actual - expected).abs() <= CAPACITY_TOLERANCE
}

const MAX_BOARD_BYTES: u64 = 1024 * 1024;
const MAX_WBS_ITEMS: usize = 100;

#[async_trait]
impl WbsAssignmentCandidatesPort for HttpApiClient {
    async fn recommend_assignees(
        &self,
        query: &WbsCandidateQuery,
    ) -> Result<WbsAssignmentCandidates, WbsCandidateError> {
        let organization = query
            .context()
            .organization_id()
            .ok_or(WbsCandidateError::InvalidInput)?;
        let organization =
            url::form_urlencoded::byte_serialize(organization.as_bytes()).collect::<String>();
        let parameters = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("wbs_item_id", query.context().wbs_item_id())
            .append_pair("limit", &query.limit().to_string())
            .finish();
        let response = self
            .authorized_request(
                reqwest::Method::GET,
                &format!("/api/v1/organizations/{organization}/assignment-board?{parameters}"),
            )
            .await
            .map_err(|_| WbsCandidateError::Unauthorized)?
            .send()
            .await
            .map_err(transport_error)?;
        match response.status().as_u16() {
            200 => {}
            401 | 403 => return Err(WbsCandidateError::Unauthorized),
            409 => return Err(WbsCandidateError::Stale),
            422 => return Err(WbsCandidateError::InvalidInput),
            429 => return Err(WbsCandidateError::RateLimited),
            _ => return Err(WbsCandidateError::Unavailable),
        }
        let bytes =
            read_body_capped(response, MAX_BOARD_BYTES)
                .await
                .map_err(|error| match error {
                    BodyReadError::Transport(error) => transport_error(error),
                    BodyReadError::TooLarge { .. } => WbsCandidateError::InvalidResponse,
                })?;
        let board: AssignmentBoardSnapshotDto =
            serde_json::from_slice(&bytes).map_err(|_| WbsCandidateError::InvalidResponse)?;
        project_board(board, query)
    }
}

fn transport_error(error: reqwest::Error) -> WbsCandidateError {
    if error.is_timeout() {
        WbsCandidateError::Timeout
    } else {
        WbsCandidateError::Unavailable
    }
}

fn project_board(
    board: AssignmentBoardSnapshotDto,
    query: &WbsCandidateQuery,
) -> Result<WbsAssignmentCandidates, WbsCandidateError> {
    let snapshot = validate_board_source(&board, query)?;
    if board.wbs_items.len() > MAX_WBS_ITEMS {
        return Err(WbsCandidateError::InvalidResponse);
    }
    let mut items = board
        .wbs_items
        .iter()
        .filter(|item| item.item_id == query.context().wbs_item_id());
    let item = items.next().ok_or(WbsCandidateError::Stale)?;
    if items.next().is_some() {
        return Err(WbsCandidateError::InvalidResponse);
    }
    let count = board.candidates.len();
    if board.page.limit != query.limit()
        || count > usize::from(query.limit())
        || u32::try_from(count).ok() != Some(board.page.returned_candidates)
        || board.page.total_candidates < board.page.returned_candidates
        || board.page.truncated != (board.page.total_candidates > board.page.returned_candidates)
        || board.page.next_cursor.as_deref()
            != board
                .candidates
                .last()
                .filter(|_| board.page.truncated)
                .map(|candidate| candidate.user_id.as_str())
    {
        return Err(WbsCandidateError::InvalidResponse);
    }
    let candidates = board
        .candidates
        .into_iter()
        .map(|candidate| {
            validate_candidate_binding(&candidate, &board.organization_id, &item.item_id)?;
            project_candidate(candidate, &item.skill_id)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let provenance = WbsCandidateProvenance::new(board.synthetic, board.provenance)?;
    // Validate all source pins, identities and rank order before filtering.
    let validated = WbsAssignmentCandidates::ready(
        query.clone(),
        snapshot.clone(),
        provenance.clone(),
        candidates,
    )?;
    let eligible: Vec<_> = validated
        .candidates()
        .iter()
        .filter(|candidate| candidate.eligible())
        .cloned()
        .collect();
    if eligible.is_empty() {
        return Err(WbsCandidateError::NoCandidates);
    }
    WbsAssignmentCandidates::ready(query.clone(), snapshot, provenance, eligible)
}

#[cfg(test)]
mod tests;
