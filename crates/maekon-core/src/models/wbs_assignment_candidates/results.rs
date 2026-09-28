//! Fixed WBS recommendation results and stale-selection validation.

use std::collections::HashSet;
use std::fmt;

use super::local::WbsLocalResultSnapshot;
use super::{
    WbsAssignmentCandidate, WbsCandidateError, WbsCandidateProvenance, WbsCandidateQuery,
    WbsCandidateSnapshot, WbsCandidateSnapshotRef, WbsCandidateSource, WbsCandidateSourceKind,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WbsCandidateState {
    Ready,
    Empty,
}

/// A fixed response bound to its original query and validated source. The
/// producer must validate authenticated server identity or explicit local input
/// provenance, including the empty case, before invoking these constructors.
#[derive(Clone, PartialEq, Eq)]
pub struct WbsAssignmentCandidates {
    query: WbsCandidateQuery,
    source: Option<WbsCandidateSource>,
    provenance: WbsCandidateProvenance,
    candidates: Vec<WbsAssignmentCandidate>,
}

impl WbsAssignmentCandidates {
    pub fn ready(
        query: WbsCandidateQuery,
        snapshot: WbsCandidateSnapshot,
        provenance: WbsCandidateProvenance,
        candidates: Vec<WbsAssignmentCandidate>,
    ) -> Result<Self, WbsCandidateError> {
        if query.context.source_kind() != WbsCandidateSourceKind::ServerBoard {
            return Err(WbsCandidateError::InvalidInput);
        }
        if query
            .context
            .expected_wbs_version_id()
            .is_some_and(|v| v != snapshot.wbs_version_id())
        {
            return Err(WbsCandidateError::Stale);
        }
        Self::validate_candidates(&query, snapshot.reference(), &candidates)?;
        Ok(Self {
            query,
            source: Some(WbsCandidateSource::ServerBoard(snapshot)),
            provenance,
            candidates,
        })
    }

    pub fn ready_local(
        query: WbsCandidateQuery,
        snapshot: WbsLocalResultSnapshot,
        provenance: WbsCandidateProvenance,
        candidates: Vec<WbsAssignmentCandidate>,
    ) -> Result<Self, WbsCandidateError> {
        let input = query
            .context
            .local_input()
            .ok_or(WbsCandidateError::InvalidInput)?;
        if input != snapshot.input() {
            return Err(WbsCandidateError::Stale);
        }
        Self::validate_candidates(&query, snapshot.reference(), &candidates)?;
        Ok(Self {
            query,
            source: Some(WbsCandidateSource::LocalDocumentRoster(snapshot)),
            provenance,
            candidates,
        })
    }

    fn validate_candidates(
        query: &WbsCandidateQuery,
        reference: &WbsCandidateSnapshotRef,
        candidates: &[WbsAssignmentCandidate],
    ) -> Result<(), WbsCandidateError> {
        if candidates.len() > usize::from(query.limit) {
            return Err(WbsCandidateError::InvalidResponse);
        }
        let mut ids = HashSet::new();
        let mut users = HashSet::new();
        let mut previous_rank = 0;
        for candidate in candidates {
            if &candidate.source_snapshot != reference
                || !ids.insert(candidate.identity.candidate_id())
                || !users.insert(candidate.identity.user_id())
                || candidate.rank <= previous_rank
            {
                return Err(WbsCandidateError::InvalidResponse);
            }
            previous_rank = candidate.rank;
        }
        Ok(())
    }

    pub fn empty(query: WbsCandidateQuery, provenance: WbsCandidateProvenance) -> Self {
        Self {
            query,
            source: None,
            provenance,
            candidates: Vec::new(),
        }
    }

    pub fn query(&self) -> &WbsCandidateQuery {
        &self.query
    }
    pub fn snapshot(&self) -> Option<&WbsCandidateSnapshot> {
        match self.source.as_ref() {
            Some(WbsCandidateSource::ServerBoard(snapshot)) => Some(snapshot),
            _ => None,
        }
    }
    pub fn local_snapshot(&self) -> Option<&WbsLocalResultSnapshot> {
        match self.source.as_ref() {
            Some(WbsCandidateSource::LocalDocumentRoster(snapshot)) => Some(snapshot),
            _ => None,
        }
    }
    pub fn source(&self) -> Option<&WbsCandidateSource> {
        self.source.as_ref()
    }
    pub fn source_reference(&self) -> Option<&WbsCandidateSnapshotRef> {
        self.source.as_ref().map(WbsCandidateSource::reference)
    }
    pub fn provenance(&self) -> &WbsCandidateProvenance {
        &self.provenance
    }
    pub fn candidates(&self) -> &[WbsAssignmentCandidate] {
        &self.candidates
    }
    pub fn state(&self) -> WbsCandidateState {
        if self.source.is_some() {
            WbsCandidateState::Ready
        } else {
            WbsCandidateState::Empty
        }
    }

    /// Select only against the current local query/generation and original source.
    /// The runtime still owns source freshness, user approval and execution gates.
    pub fn select(
        &self,
        current_query: &WbsCandidateQuery,
        source: &WbsCandidateSnapshotRef,
        candidate_id: &str,
    ) -> Result<&WbsAssignmentCandidate, WbsCandidateError> {
        if current_query != &self.query {
            return Err(WbsCandidateError::Stale);
        }
        let original_source = self
            .source_reference()
            .ok_or(WbsCandidateError::NoCandidates)?;
        if source != original_source {
            return Err(WbsCandidateError::Stale);
        }
        let candidate = self
            .candidates
            .iter()
            .find(|candidate| candidate.identity.candidate_id() == candidate_id)
            .ok_or(WbsCandidateError::NoCandidates)?;
        if !candidate.eligible {
            return Err(WbsCandidateError::Ineligible);
        }
        Ok(candidate)
    }
}

redacted_debug!(WbsAssignmentCandidates);
