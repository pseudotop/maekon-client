//! Validated snapshot, candidate identity and algorithm provenance (#12140).
//! Shape validation is not authentication, consent, policy or cell-write authority.

use std::fmt;

use chrono::{DateTime, Utc};

#[derive(Clone, PartialEq, Eq)]
pub struct WbsCandidateSnapshotRef {
    snapshot_id: String,
    snapshot_version: String,
    snapshot_hash: String,
}

impl WbsCandidateSnapshotRef {
    pub fn new(
        snapshot_id: String,
        snapshot_version: String,
        snapshot_hash: String,
    ) -> Result<Self, WbsCandidateError> {
        if !valid_id(&snapshot_id) || !valid_id(&snapshot_version) || !valid_hash(&snapshot_hash) {
            return Err(WbsCandidateError::InvalidResponse);
        }
        Ok(Self {
            snapshot_id,
            snapshot_version,
            snapshot_hash,
        })
    }

    pub fn snapshot_id(&self) -> &str {
        &self.snapshot_id
    }
    pub fn snapshot_version(&self) -> &str {
        &self.snapshot_version
    }
    pub fn snapshot_hash(&self) -> &str {
        &self.snapshot_hash
    }
}

/// Board identity and approved WBS identity are distinct; their hashes are not interchangeable.
#[derive(Clone, PartialEq, Eq)]
pub struct WbsCandidateSnapshot {
    reference: WbsCandidateSnapshotRef,
    wbs_version_id: String,
    wbs_content_hash: String,
    approval_id: String,
}

impl WbsCandidateSnapshot {
    pub fn new(
        reference: WbsCandidateSnapshotRef,
        wbs_version_id: String,
        wbs_content_hash: String,
        approval_id: String,
    ) -> Result<Self, WbsCandidateError> {
        if !valid_id(&wbs_version_id) || !valid_hash(&wbs_content_hash) || !valid_id(&approval_id) {
            return Err(WbsCandidateError::InvalidResponse);
        }
        Ok(Self {
            reference,
            wbs_version_id,
            wbs_content_hash,
            approval_id,
        })
    }

    pub fn reference(&self) -> &WbsCandidateSnapshotRef {
        &self.reference
    }
    pub fn wbs_version_id(&self) -> &str {
        &self.wbs_version_id
    }
    pub fn wbs_content_hash(&self) -> &str {
        &self.wbs_content_hash
    }
    pub fn approval_id(&self) -> &str {
        &self.approval_id
    }
}

/// Preserve the producer's algorithm provenance.
#[derive(Clone, PartialEq, Eq)]
pub struct WbsCandidateAlgorithm {
    algorithm_id: String,
    algorithm_version: String,
    as_of: DateTime<Utc>,
}

impl WbsCandidateAlgorithm {
    pub fn new(
        algorithm_id: String,
        algorithm_version: String,
        as_of: DateTime<Utc>,
    ) -> Result<Self, WbsCandidateError> {
        if !valid_id(&algorithm_id) || !valid_id(&algorithm_version) {
            return Err(WbsCandidateError::InvalidResponse);
        }
        Ok(Self {
            algorithm_id,
            algorithm_version,
            as_of,
        })
    }

    pub fn algorithm_id(&self) -> &str {
        &self.algorithm_id
    }
    pub fn algorithm_version(&self) -> &str {
        &self.algorithm_version
    }
    pub fn as_of(&self) -> DateTime<Utc> {
        self.as_of
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct WbsCandidateIdentity {
    candidate_id: String,
    candidate_hash: String,
    user_id: String,
}

impl WbsCandidateIdentity {
    pub fn new(
        candidate_id: String,
        candidate_hash: String,
        user_id: String,
    ) -> Result<Self, WbsCandidateError> {
        if !valid_id(&candidate_id) || !valid_hash(&candidate_hash) || !valid_id(&user_id) {
            return Err(WbsCandidateError::InvalidResponse);
        }
        Ok(Self {
            candidate_id,
            candidate_hash,
            user_id,
        })
    }

    pub fn candidate_id(&self) -> &str {
        &self.candidate_id
    }
    pub fn candidate_hash(&self) -> &str {
        &self.candidate_hash
    }
    pub fn user_id(&self) -> &str {
        &self.user_id
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum WbsCandidateError {
    #[error("wbs_candidates_invalid_input")]
    InvalidInput,
    #[error("wbs_candidates_invalid_response")]
    InvalidResponse,
    #[error("wbs_candidates_unsupported_contract")]
    UnsupportedContract,
    #[error("wbs_candidates_stale")]
    Stale,
    #[error("wbs_candidates_unavailable")]
    Unavailable,
    #[error("wbs_candidates_unauthorized")]
    Unauthorized,
    #[error("wbs_candidates_rate_limited")]
    RateLimited,
    #[error("wbs_candidates_timeout")]
    Timeout,
    #[error("wbs_candidates_empty")]
    NoCandidates,
    #[error("wbs_candidate_ineligible")]
    Ineligible,
}

// Bounded identifiers shared by WBS model contracts.
pub(super) fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b))
}

fn valid_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

macro_rules! redacted_debug {
    ($($ty:ty),+ $(,)?) => { $(
        impl fmt::Debug for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(concat!(stringify!($ty), "([REDACTED])"))
            }
        }
    )+ };
}
redacted_debug!(
    WbsCandidateSnapshotRef,
    WbsCandidateSnapshot,
    WbsCandidateAlgorithm,
    WbsCandidateIdentity
);

pub mod local;

#[derive(Clone, PartialEq, Eq)]
pub struct WbsCandidateProvenance {
    synthetic: bool,
    sources: Vec<String>,
}

impl WbsCandidateProvenance {
    pub fn new(synthetic: bool, sources: Vec<String>) -> Result<Self, WbsCandidateError> {
        if sources.len() > 16 || sources.iter().any(|value| !valid_text(value, 256, false)) {
            return Err(WbsCandidateError::InvalidResponse);
        }
        Ok(Self { synthetic, sources })
    }

    pub fn synthetic(&self) -> bool {
        self.synthetic
    }
    pub fn sources(&self) -> &[String] {
        &self.sources
    }
}

// Shared with cell snapshots so text bounds use bytes and never normalize old values.
pub(super) fn valid_text(value: &str, max_bytes: usize, allow_empty: bool) -> bool {
    (allow_empty || !value.is_empty())
        && value.len() <= max_bytes
        && !value.chars().any(char::is_control)
}

redacted_debug!(WbsCandidateProvenance);

/// Display text is untrusted data, not a command, path, formula or writable cell address.
#[derive(Clone, PartialEq, Eq)]
pub struct WbsAssignmentCandidate {
    identity: WbsCandidateIdentity,
    display_name: String,
    rank: u32,
    eligible: bool,
    rank_reason: String,
    source_snapshot: WbsCandidateSnapshotRef,
    algorithm: WbsCandidateAlgorithm,
}

impl WbsAssignmentCandidate {
    pub fn new(
        identity: WbsCandidateIdentity,
        display_name: String,
        rank: u32,
        eligible: bool,
        rank_reason: String,
        source_snapshot: WbsCandidateSnapshotRef,
        algorithm: WbsCandidateAlgorithm,
    ) -> Result<Self, WbsCandidateError> {
        if rank == 0
            || !valid_text(&display_name, 256, false)
            || !valid_text(&rank_reason, 512, false)
        {
            return Err(WbsCandidateError::InvalidResponse);
        }
        Ok(Self {
            identity,
            display_name,
            rank,
            eligible,
            rank_reason,
            source_snapshot,
            algorithm,
        })
    }

    pub fn identity(&self) -> &WbsCandidateIdentity {
        &self.identity
    }
    pub fn display_name(&self) -> &str {
        &self.display_name
    }
    pub fn rank(&self) -> u32 {
        self.rank
    }
    pub fn eligible(&self) -> bool {
        self.eligible
    }
    pub fn rank_reason(&self) -> &str {
        &self.rank_reason
    }
    pub fn source_snapshot(&self) -> &WbsCandidateSnapshotRef {
        &self.source_snapshot
    }
    pub fn algorithm(&self) -> &WbsCandidateAlgorithm {
        &self.algorithm
    }
}

redacted_debug!(WbsAssignmentCandidate);

use local::{WbsLocalInputRevision, WbsLocalResultSnapshot};
use serde::Serialize;

pub const WBS_ASSIGNMENT_CONTRACT_VERSION: &str = "assignment-board.v1";
pub const MAX_WBS_CANDIDATES: u8 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WbsCandidateSourceKind {
    ServerBoard,
    LocalDocumentRoster,
}

/// Native-owned input source and selected stable WBS item. A local input has no
/// organization or server approval identity.
#[derive(Clone, PartialEq, Eq)]
pub struct WbsAssignmentContext {
    organization_id: Option<String>,
    wbs_item_id: String,
    expected_wbs_version_id: Option<String>,
    local_input: Option<WbsLocalInputRevision>,
}

impl WbsAssignmentContext {
    pub fn new(
        organization_id: String,
        wbs_item_id: String,
        expected_wbs_version_id: Option<String>,
    ) -> Result<Self, WbsCandidateError> {
        if !valid_id(&organization_id)
            || !valid_id(&wbs_item_id)
            || expected_wbs_version_id
                .as_deref()
                .is_some_and(|id| !valid_id(id))
        {
            return Err(WbsCandidateError::InvalidInput);
        }
        Ok(Self {
            organization_id: Some(organization_id),
            wbs_item_id,
            expected_wbs_version_id,
            local_input: None,
        })
    }

    pub fn new_local(
        input: WbsLocalInputRevision,
        wbs_item_id: String,
    ) -> Result<Self, WbsCandidateError> {
        if !valid_id(&wbs_item_id) {
            return Err(WbsCandidateError::InvalidInput);
        }
        Ok(Self {
            organization_id: None,
            wbs_item_id,
            expected_wbs_version_id: None,
            local_input: Some(input),
        })
    }

    pub fn organization_id(&self) -> Option<&str> {
        self.organization_id.as_deref()
    }
    pub fn wbs_item_id(&self) -> &str {
        &self.wbs_item_id
    }
    pub fn expected_wbs_version_id(&self) -> Option<&str> {
        self.expected_wbs_version_id.as_deref()
    }
    pub fn local_input(&self) -> Option<&WbsLocalInputRevision> {
        self.local_input.as_ref()
    }
    pub fn source_kind(&self) -> WbsCandidateSourceKind {
        if self.local_input.is_some() {
            WbsCandidateSourceKind::LocalDocumentRoster
        } else {
            WbsCandidateSourceKind::ServerBoard
        }
    }
}

/// Generation belongs to the requesting local interaction, not to server authority.
#[derive(Clone, PartialEq, Eq)]
pub struct WbsCandidateQuery {
    context: WbsAssignmentContext,
    generation: u64,
    limit: u8,
}

impl WbsCandidateQuery {
    pub fn new(
        context: WbsAssignmentContext,
        generation: u64,
        limit: u8,
    ) -> Result<Self, WbsCandidateError> {
        if generation == 0 || !(1..=MAX_WBS_CANDIDATES).contains(&limit) {
            return Err(WbsCandidateError::InvalidInput);
        }
        Ok(Self {
            context,
            generation,
            limit,
        })
    }

    pub fn context(&self) -> &WbsAssignmentContext {
        &self.context
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn limit(&self) -> u8 {
        self.limit
    }
}

/// Disjoint source identities prevent local results from inventing server pins.
#[derive(Clone, PartialEq, Eq)]
pub enum WbsCandidateSource {
    ServerBoard(WbsCandidateSnapshot),
    LocalDocumentRoster(WbsLocalResultSnapshot),
}

impl WbsCandidateSource {
    pub fn kind(&self) -> WbsCandidateSourceKind {
        match self {
            Self::ServerBoard(_) => WbsCandidateSourceKind::ServerBoard,
            Self::LocalDocumentRoster(_) => WbsCandidateSourceKind::LocalDocumentRoster,
        }
    }
    pub fn reference(&self) -> &WbsCandidateSnapshotRef {
        match self {
            Self::ServerBoard(snapshot) => snapshot.reference(),
            Self::LocalDocumentRoster(snapshot) => snapshot.reference(),
        }
    }
}

redacted_debug!(WbsAssignmentContext, WbsCandidateQuery, WbsCandidateSource);

mod results;
pub use results::{WbsAssignmentCandidates, WbsCandidateState};

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn reference() -> Result<WbsCandidateSnapshotRef, WbsCandidateError> {
        WbsCandidateSnapshotRef::new("board-private".into(), "v1".into(), "a".repeat(64))
    }

    #[test]
    fn identifiers_accept_the_exact_boundary_and_reject_unsupported_text() {
        for id in ["a", "9", ".", "_", ":", "-", "Board_1.v2:row-a"] {
            assert!(valid_id(id));
        }
        assert!(valid_id(&"i".repeat(128)));
        for id in [
            "".to_owned(),
            "i".repeat(129),
            "has space".into(),
            "a/b".into(),
            "line\nbreak".into(),
            "한글".into(),
        ] {
            assert!(!valid_id(&id));
        }
    }

    #[test]
    fn hashes_require_exactly_64_hexadecimal_bytes() {
        assert!(valid_hash(&"0".repeat(64)));
        assert!(valid_hash(&"aB09".repeat(16)));
        for hash in [
            "".to_owned(),
            "a".repeat(63),
            "a".repeat(65),
            "g".repeat(64),
            format!("{}g", "a".repeat(63)),
        ] {
            assert!(!valid_hash(&hash));
        }
    }

    #[test]
    fn snapshot_reference_preserves_each_pin_and_rejects_each_invalid_dimension() -> TestResult {
        let reference = reference()?;
        assert_eq!(reference.snapshot_id(), "board-private");
        assert_eq!(reference.snapshot_version(), "v1");
        assert_eq!(reference.snapshot_hash(), "a".repeat(64));
        for (id, version, hash) in [
            ("".into(), "v1".into(), "a".repeat(64)),
            ("board".into(), "".into(), "a".repeat(64)),
            ("board".into(), "v1".into(), "z".repeat(64)),
        ] {
            assert_eq!(
                WbsCandidateSnapshotRef::new(id, version, hash),
                Err(WbsCandidateError::InvalidResponse)
            );
        }
        Ok(())
    }

    #[test]
    fn snapshot_keeps_board_and_approved_wbs_identity_distinct() -> TestResult {
        let reference = reference()?;
        let snapshot = WbsCandidateSnapshot::new(
            reference.clone(),
            "wbs-v2".into(),
            "b".repeat(64),
            "approval-private".into(),
        )?;
        assert_eq!(snapshot.reference(), &reference);
        assert_eq!(snapshot.wbs_version_id(), "wbs-v2");
        assert_eq!(snapshot.wbs_content_hash(), "b".repeat(64));
        assert_eq!(snapshot.approval_id(), "approval-private");
        for (version, hash, approval) in [
            ("".into(), "b".repeat(64), "approval".into()),
            ("wbs-v2".into(), "z".repeat(64), "approval".into()),
            ("wbs-v2".into(), "b".repeat(64), "".into()),
        ] {
            assert_eq!(
                WbsCandidateSnapshot::new(reference.clone(), version, hash, approval),
                Err(WbsCandidateError::InvalidResponse)
            );
        }
        Ok(())
    }

    #[test]
    fn algorithm_preserves_the_producer_version_and_timestamp() -> TestResult {
        let as_of = DateTime::parse_from_rfc3339("2026-07-01T12:34:56Z")?.with_timezone(&Utc);
        let algorithm = WbsCandidateAlgorithm::new("ranker-private".into(), "1.2.3".into(), as_of)?;
        assert_eq!(algorithm.algorithm_id(), "ranker-private");
        assert_eq!(algorithm.algorithm_version(), "1.2.3");
        assert_eq!(algorithm.as_of(), as_of);
        for (id, version) in [("", "1.2.3"), ("ranker", "")] {
            assert_eq!(
                WbsCandidateAlgorithm::new(id.into(), version.into(), as_of),
                Err(WbsCandidateError::InvalidResponse)
            );
        }
        Ok(())
    }

    #[test]
    fn candidate_identity_preserves_its_pins_and_rejects_each_invalid_dimension() -> TestResult {
        let identity = WbsCandidateIdentity::new(
            "candidate-private".into(),
            "c".repeat(64),
            "user-private".into(),
        )?;
        assert_eq!(identity.candidate_id(), "candidate-private");
        assert_eq!(identity.candidate_hash(), "c".repeat(64));
        assert_eq!(identity.user_id(), "user-private");
        for (id, hash, user) in [
            ("".into(), "c".repeat(64), "user".into()),
            ("candidate".into(), "z".repeat(64), "user".into()),
            ("candidate".into(), "c".repeat(64), "".into()),
        ] {
            assert_eq!(
                WbsCandidateIdentity::new(id, hash, user),
                Err(WbsCandidateError::InvalidResponse)
            );
        }
        Ok(())
    }

    #[test]
    fn debug_redacts_each_snapshot_identity_and_algorithm_value() -> TestResult {
        let reference = reference()?;
        let snapshot = WbsCandidateSnapshot::new(
            reference.clone(),
            "wbs-v2".into(),
            "b".repeat(64),
            "approval-private".into(),
        )?;
        let algorithm = WbsCandidateAlgorithm::new(
            "ranker-private".into(),
            "1.2.3".into(),
            DateTime::parse_from_rfc3339("2026-07-01T12:34:56Z")?.with_timezone(&Utc),
        )?;
        let identity = WbsCandidateIdentity::new(
            "candidate-private".into(),
            "c".repeat(64),
            "user-private".into(),
        )?;
        let output = format!("{reference:?} {snapshot:?} {algorithm:?} {identity:?}");
        for private in [
            "board-private",
            "v1",
            "wbs-v2",
            "approval-private",
            "ranker-private",
            "1.2.3",
            "2026-07-01",
            "candidate-private",
            "user-private",
        ] {
            assert!(!output.contains(private));
        }
        for hash in ["a".repeat(64), "b".repeat(64), "c".repeat(64)] {
            assert!(!output.contains(&hash));
        }
        for name in [
            "WbsCandidateSnapshotRef",
            "WbsCandidateSnapshot",
            "WbsCandidateAlgorithm",
            "WbsCandidateIdentity",
        ] {
            assert!(output.contains(&format!("{name}([REDACTED])")));
        }
        Ok(())
    }

    #[test]
    fn text_bounds_use_bytes_and_the_explicit_empty_policy() {
        assert!(valid_text("", 0, true));
        assert!(!valid_text("", 0, false));
        assert!(valid_text("ab", 2, false));
        assert!(!valid_text("abc", 2, false));
        assert!(valid_text("한", 3, false));
        assert!(!valid_text("한", 2, false));
        assert!(!valid_text("a\n", 2, false));
        assert!(!valid_text("a\t", 2, false));
    }

    #[test]
    fn provenance_preserves_empty_and_boundary_sources_without_inventing_facts() -> TestResult {
        let sources = vec!["s".repeat(256); 16];
        let provenance = WbsCandidateProvenance::new(true, sources.clone())?;
        assert!(provenance.synthetic());
        assert_eq!(provenance.sources(), sources);
        let empty = WbsCandidateProvenance::new(false, Vec::new())?;
        assert!(!empty.synthetic());
        assert!(empty.sources().is_empty());
        for invalid in [
            vec!["s".into(); 17],
            vec!["s".repeat(257)],
            vec![String::new()],
            vec!["private\nsource".into()],
        ] {
            assert_eq!(
                WbsCandidateProvenance::new(false, invalid),
                Err(WbsCandidateError::InvalidResponse)
            );
        }
        Ok(())
    }

    #[test]
    fn provenance_debug_redacts_actual_source_text() -> TestResult {
        let provenance = WbsCandidateProvenance::new(false, vec!["private-source".into()])?;
        assert_eq!(provenance.sources(), ["private-source"]);
        assert_eq!(
            format!("{provenance:?}"),
            "WbsCandidateProvenance([REDACTED])"
        );
        Ok(())
    }

    #[test]
    fn assignment_candidate_preserves_identity_rank_and_eligibility() -> TestResult {
        let identity = WbsCandidateIdentity::new(
            "candidate-private".into(),
            "c".repeat(64),
            "user-private".into(),
        )?;
        let source = reference()?;
        let algorithm = WbsCandidateAlgorithm::new(
            "ranker-private".into(),
            "1.2.3".into(),
            DateTime::parse_from_rfc3339("2026-07-01T12:34:56Z")?.with_timezone(&Utc),
        )?;
        for eligible in [false, true] {
            let candidate = WbsAssignmentCandidate::new(
                identity.clone(),
                "Private Candidate".into(),
                2,
                eligible,
                "skill_match".into(),
                source.clone(),
                algorithm.clone(),
            )?;
            assert_eq!(candidate.identity(), &identity);
            assert_eq!(candidate.display_name(), "Private Candidate");
            assert_eq!(candidate.rank(), 2);
            assert_eq!(candidate.eligible(), eligible);
            assert_eq!(candidate.rank_reason(), "skill_match");
            assert_eq!(candidate.source_snapshot(), &source);
            assert_eq!(candidate.algorithm(), &algorithm);
            assert_eq!(
                format!("{candidate:?}"),
                "WbsAssignmentCandidate([REDACTED])"
            );
        }
        Ok(())
    }

    #[test]
    fn assignment_candidate_rejects_zero_rank_and_each_unbounded_text_field() -> TestResult {
        let identity =
            WbsCandidateIdentity::new("candidate".into(), "c".repeat(64), "user".into())?;
        let source = reference()?;
        let algorithm = WbsCandidateAlgorithm::new(
            "ranker".into(),
            "v1".into(),
            DateTime::parse_from_rfc3339("2026-07-01T12:34:56Z")?.with_timezone(&Utc),
        )?;
        let boundary = WbsAssignmentCandidate::new(
            identity.clone(),
            "n".repeat(256),
            1,
            true,
            "r".repeat(512),
            source.clone(),
            algorithm.clone(),
        )?;
        assert_eq!(boundary.display_name().len(), 256);
        assert_eq!(boundary.rank_reason().len(), 512);
        for (rank, name, reason) in [
            (0, "Candidate".into(), "reason".into()),
            (1, String::new(), "reason".into()),
            (1, "n".repeat(257), "reason".into()),
            (1, "Private\nCandidate".into(), "reason".into()),
            (1, "Candidate".into(), String::new()),
            (1, "Candidate".into(), "r".repeat(513)),
            (1, "Candidate".into(), "private\nreason".into()),
        ] {
            assert_eq!(
                WbsAssignmentCandidate::new(
                    identity.clone(),
                    name,
                    rank,
                    true,
                    reason,
                    source.clone(),
                    algorithm.clone()
                ),
                Err(WbsCandidateError::InvalidResponse)
            );
        }
        Ok(())
    }

    fn local_revision() -> Result<WbsLocalInputRevision, WbsCandidateError> {
        WbsLocalInputRevision::new(
            "document-a".into(),
            "input-v1".into(),
            "a".repeat(64),
            "roster-v1".into(),
            "b".repeat(64),
        )
    }

    #[test]
    fn contexts_keep_local_input_disjoint_from_server_identity() -> TestResult {
        let server =
            WbsAssignmentContext::new("org-a".into(), "row-server".into(), Some("wbs-v2".into()))?;
        assert_eq!(server.organization_id(), Some("org-a"));
        assert_eq!(server.wbs_item_id(), "row-server");
        assert_eq!(server.expected_wbs_version_id(), Some("wbs-v2"));
        assert_eq!(server.local_input(), None);
        assert_eq!(server.source_kind(), WbsCandidateSourceKind::ServerBoard);
        let local = WbsAssignmentContext::new_local(local_revision()?, "row-local".into())?;
        assert_eq!(local.organization_id(), None);
        assert_eq!(local.expected_wbs_version_id(), None);
        assert_eq!(local.wbs_item_id(), "row-local");
        assert_eq!(local.local_input(), Some(&local_revision()?));
        assert_eq!(
            local.source_kind(),
            WbsCandidateSourceKind::LocalDocumentRoster
        );
        for (org, row, version) in [
            ("", "row", None),
            ("org", "", None),
            ("org", "row", Some("bad/version")),
        ] {
            assert_eq!(
                WbsAssignmentContext::new(org.into(), row.into(), version.map(str::to_owned)),
                Err(WbsCandidateError::InvalidInput)
            );
        }
        assert_eq!(
            WbsAssignmentContext::new_local(local_revision()?, String::new()),
            Err(WbsCandidateError::InvalidInput)
        );
        assert_eq!(format!("{local:?}"), "WbsAssignmentContext([REDACTED])");
        Ok(())
    }

    #[test]
    fn query_preserves_generation_and_accepts_only_the_bounded_candidate_limit() -> TestResult {
        let context = WbsAssignmentContext::new_local(local_revision()?, "row-a".into())?;
        for limit in [1, 10] {
            let query = WbsCandidateQuery::new(context.clone(), 7, limit)?;
            assert_eq!(query.context(), &context);
            assert_eq!(query.generation(), 7);
            assert_eq!(query.limit(), limit);
            assert_eq!(format!("{query:?}"), "WbsCandidateQuery([REDACTED])");
        }
        for (generation, limit) in [(0, 1), (1, 0), (1, 11)] {
            assert_eq!(
                WbsCandidateQuery::new(context.clone(), generation, limit),
                Err(WbsCandidateError::InvalidInput)
            );
        }
        Ok(())
    }

    #[test]
    fn source_kind_and_reference_preserve_the_distinct_result_identity() -> TestResult {
        let server_ref = reference()?;
        let local_ref =
            WbsCandidateSnapshotRef::new("local-result".into(), "r2".into(), "d".repeat(64))?;
        let server = WbsCandidateSource::ServerBoard(WbsCandidateSnapshot::new(
            server_ref.clone(),
            "wbs-v2".into(),
            "b".repeat(64),
            "approval-a".into(),
        )?);
        let provenance = local::WbsLocalModelProvenance::new(
            "c".repeat(64),
            "provider-a".into(),
            "model-a".into(),
            "prompt-v1".into(),
        )?;
        let local = WbsCandidateSource::LocalDocumentRoster(WbsLocalResultSnapshot::new(
            local_ref.clone(),
            local_revision()?,
            provenance,
        )?);
        assert_eq!(server.kind(), WbsCandidateSourceKind::ServerBoard);
        assert_eq!(server.reference(), &server_ref);
        assert_eq!(local.kind(), WbsCandidateSourceKind::LocalDocumentRoster);
        assert_eq!(local.reference(), &local_ref);
        assert_eq!(
            format!("{server:?} {local:?}"),
            "WbsCandidateSource([REDACTED]) WbsCandidateSource([REDACTED])"
        );
        Ok(())
    }
}
