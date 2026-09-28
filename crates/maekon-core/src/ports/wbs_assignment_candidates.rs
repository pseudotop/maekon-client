//! Read-only WBS assignee recommendation source; no reservation or cell-write authority.

use async_trait::async_trait;

use crate::models::wbs_assignment_candidates::{
    WbsAssignmentCandidates, WbsCandidateError, WbsCandidateQuery,
};

#[async_trait]
pub trait WbsAssignmentCandidatesPort: Send + Sync {
    /// The runtime owns authenticated server identity or explicit local input
    /// registration, provider selection and all egress gates.
    /// Implementations must bind the response to this exact WBS item/context,
    /// preserve source snapshot provenance, and reject stale/malformed responses.
    /// An empty result is distinct from unauthorized, unavailable, or stale data.
    /// This read-only operation must never reserve capacity, confirm an assignment,
    /// or send mail. A local model source must be explicitly selected and must not
    /// be an implicit fallback for server failure.
    async fn recommend_assignees(
        &self,
        query: &WbsCandidateQuery,
    ) -> Result<WbsAssignmentCandidates, WbsCandidateError>;

    /// Revalidate the original, fixed result without changing it. The default
    /// preserves the server GET/equality contract. A local provider must override
    /// this with registration/provider revision and result-cache checks, making
    /// zero model calls. Missing or changed local state is not regenerated.
    async fn revalidate_assignees(
        &self,
        original: &WbsAssignmentCandidates,
    ) -> Result<(), WbsCandidateError> {
        if original.query().context().local_input().is_some() {
            return Err(WbsCandidateError::UnsupportedContract);
        }
        let current = self.recommend_assignees(original.query()).await?;
        if current != *original {
            return Err(WbsCandidateError::Stale);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::WbsAssignmentCandidatesPort;
    use crate::models::wbs_assignment_candidates::local::*;
    use crate::models::wbs_assignment_candidates::*;
    use chrono::{DateTime, Utc};

    type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

    fn context(item: &str) -> TestResult<WbsAssignmentContext> {
        Ok(WbsAssignmentContext::new(
            "org-fixture".into(),
            item.into(),
            Some("v2".into()),
        )?)
    }

    fn query() -> TestResult<WbsCandidateQuery> {
        Ok(WbsCandidateQuery::new(context("row-a")?, 1, 10)?)
    }

    fn reference() -> TestResult<WbsCandidateSnapshotRef> {
        Ok(WbsCandidateSnapshotRef::new(
            "board-fixture".into(),
            "v1".into(),
            "a".repeat(64),
        )?)
    }

    fn snapshot() -> TestResult<WbsCandidateSnapshot> {
        Ok(WbsCandidateSnapshot::new(
            reference()?,
            "v2".into(),
            "b".repeat(64),
            "approval-a".into(),
        )?)
    }

    fn provenance() -> TestResult<WbsCandidateProvenance> {
        Ok(WbsCandidateProvenance::new(
            true,
            vec!["fixture-source".into()],
        )?)
    }

    fn candidate(
        rank: u32,
        id: &str,
        user: &str,
        eligible: bool,
    ) -> TestResult<WbsAssignmentCandidate> {
        let identity = WbsCandidateIdentity::new(id.into(), format!("{rank:064x}"), user.into())?;
        let algorithm = WbsCandidateAlgorithm::new(
            "ranker-fixture".into(),
            "1.0.0".into(),
            DateTime::parse_from_rfc3339("2026-07-01T00:00:00Z")?.with_timezone(&Utc),
        )?;
        Ok(WbsAssignmentCandidate::new(
            identity,
            "Private Candidate".into(),
            rank,
            eligible,
            "skill_match".into(),
            reference()?,
            algorithm,
        )?)
    }

    #[test]
    fn ready_returns_requested_limit_and_candidate_contents_for_both_sources() -> TestResult {
        for limit in [1, MAX_WBS_CANDIDATES] {
            let candidates = (1..=limit)
                .map(|rank| {
                    candidate(
                        u32::from(rank),
                        &format!("candidate-{rank}"),
                        &format!("user-{rank}"),
                        true,
                    )
                })
                .collect::<TestResult<Vec<_>>>()?;
            let server_query = WbsCandidateQuery::new(context("row-a")?, 1, limit)?;
            let local_query = WbsCandidateQuery::new(local_query()?.context().clone(), 1, limit)?;
            for board in [
                WbsAssignmentCandidates::ready(
                    server_query,
                    snapshot()?,
                    provenance()?,
                    candidates.clone(),
                )?,
                WbsAssignmentCandidates::ready_local(
                    local_query,
                    local_snapshot()?,
                    provenance()?,
                    candidates.clone(),
                )?,
            ] {
                assert_eq!(board.candidates(), candidates.as_slice());
            }
        }
        Ok(())
    }

    #[test]
    fn ready_preserves_provenance_and_selects_only_the_current_eligible_candidate() -> TestResult {
        let query = query()?;
        let board = WbsAssignmentCandidates::ready(
            query.clone(),
            snapshot()?,
            provenance()?,
            vec![
                candidate(1, "candidate-a", "user-a", true)?,
                candidate(2, "candidate-b", "user-b", false)?,
            ],
        )?;
        assert_eq!(board.state(), WbsCandidateState::Ready);
        assert_eq!(board.query(), &query);
        assert!(board.provenance().synthetic());
        assert_eq!(board.provenance().sources(), &["fixture-source"]);
        let selected = board.select(&query, &reference()?, "candidate-a")?;
        assert_eq!(selected.identity().user_id(), "user-a");
        assert_eq!(selected.rank_reason(), "skill_match");
        assert_eq!(selected.algorithm().algorithm_id(), "ranker-fixture");
        assert_eq!(
            board.select(&query, &reference()?, "candidate-b"),
            Err(WbsCandidateError::Ineligible)
        );
        assert_eq!(
            board.select(&query, &reference()?, "unknown"),
            Err(WbsCandidateError::NoCandidates)
        );
        Ok(())
    }

    #[test]
    fn stale_row_org_generation_or_source_cannot_select_a_same_named_candidate() -> TestResult {
        let query = query()?;
        let board = WbsAssignmentCandidates::ready(
            query.clone(),
            snapshot()?,
            provenance()?,
            vec![candidate(1, "candidate-a", "user-a", true)?],
        )?;
        let changed_queries = [
            WbsCandidateQuery::new(context("row-b")?, 1, 10)?,
            WbsCandidateQuery::new(query.context().clone(), 2, 10)?,
            WbsCandidateQuery::new(
                WbsAssignmentContext::new("other-org".into(), "row-a".into(), Some("v2".into()))?,
                1,
                10,
            )?,
        ];
        for changed in changed_queries {
            assert_eq!(
                board.select(&changed, &reference()?, "candidate-a"),
                Err(WbsCandidateError::Stale)
            );
        }
        for changed in [
            WbsCandidateSnapshotRef::new("other-board".into(), "v1".into(), "a".repeat(64))?,
            WbsCandidateSnapshotRef::new("board-fixture".into(), "v2".into(), "a".repeat(64))?,
            WbsCandidateSnapshotRef::new("board-fixture".into(), "v1".into(), "c".repeat(64))?,
        ] {
            assert_eq!(
                board.select(&query, &changed, "candidate-a"),
                Err(WbsCandidateError::Stale)
            );
        }
        Ok(())
    }

    #[test]
    fn ready_rejects_stale_wbs_and_candidate_source_pins() -> TestResult {
        let stale = WbsCandidateSnapshot::new(
            reference()?,
            "v3".into(),
            "b".repeat(64),
            "approval-a".into(),
        )?;
        assert_eq!(
            WbsAssignmentCandidates::ready(query()?, stale, provenance()?, Vec::new()),
            Err(WbsCandidateError::Stale)
        );
        let other = WbsCandidateSnapshot::new(
            WbsCandidateSnapshotRef::new("other-board".into(), "v1".into(), "a".repeat(64))?,
            "v2".into(),
            "b".repeat(64),
            "approval-a".into(),
        )?;
        assert_eq!(
            WbsAssignmentCandidates::ready(
                query()?,
                other,
                provenance()?,
                vec![candidate(1, "candidate-a", "user-a", true)?]
            ),
            Err(WbsCandidateError::InvalidResponse)
        );
        let accepted = WbsAssignmentCandidates::ready(
            query()?,
            snapshot()?,
            provenance()?,
            vec![candidate(1, "candidate-a", "user-a", true)?],
        )?;
        assert_eq!(accepted.snapshot(), Some(&snapshot()?));
        assert_eq!(
            accepted.select(&query()?, &reference()?, "candidate-a")?,
            &candidate(1, "candidate-a", "user-a", true)?
        );
        Ok(())
    }

    #[test]
    fn duplicate_candidates_users_and_rank_order_are_rejected_independently() -> TestResult {
        for second in [
            candidate(2, "candidate-a", "user-b", true)?,
            candidate(2, "candidate-b", "user-a", true)?,
            candidate(1, "candidate-b", "user-b", true)?,
        ] {
            assert_eq!(
                WbsAssignmentCandidates::ready(
                    query()?,
                    snapshot()?,
                    provenance()?,
                    vec![candidate(1, "candidate-a", "user-a", true)?, second]
                ),
                Err(WbsCandidateError::InvalidResponse)
            );
        }
        assert_eq!(
            WbsAssignmentCandidates::ready(
                query()?,
                snapshot()?,
                provenance()?,
                vec![
                    candidate(2, "candidate-b", "user-b", true)?,
                    candidate(1, "candidate-a", "user-a", true)?
                ]
            ),
            Err(WbsCandidateError::InvalidResponse)
        );
        let one = WbsCandidateQuery::new(context("row-a")?, 1, 1)?;
        assert_eq!(
            WbsAssignmentCandidates::ready(
                one,
                snapshot()?,
                provenance()?,
                vec![
                    candidate(1, "candidate-a", "user-a", true)?,
                    candidate(2, "candidate-b", "user-b", true)?
                ]
            ),
            Err(WbsCandidateError::InvalidResponse)
        );
        Ok(())
    }

    #[test]
    fn empty_keeps_query_and_provenance_without_inventing_a_snapshot() -> TestResult {
        let query = query()?;
        let empty = WbsAssignmentCandidates::empty(query.clone(), provenance()?);
        assert_eq!(empty.state(), WbsCandidateState::Empty);
        assert_eq!(empty.query(), &query);
        assert!(empty.snapshot().is_none());
        assert!(empty.candidates().is_empty());
        assert!(empty.provenance().synthetic());
        assert_eq!(
            empty.select(&query, &reference()?, "candidate-a"),
            Err(WbsCandidateError::NoCandidates)
        );
        let no_matches =
            WbsAssignmentCandidates::ready(query, snapshot()?, provenance()?, Vec::new())?;
        assert_eq!(no_matches.state(), WbsCandidateState::Ready);
        assert!(no_matches.snapshot().is_some());
        Ok(())
    }

    #[test]
    fn bounded_inputs_accept_limits_and_reject_malformed_identifiers_and_hashes() -> TestResult {
        let boundary_context = WbsAssignmentContext::new("o".repeat(128), "row".into(), None)?;
        assert_eq!(
            boundary_context.organization_id(),
            Some("o".repeat(128).as_str())
        );
        assert_eq!(boundary_context.wbs_item_id(), "row");
        assert_eq!(boundary_context.expected_wbs_version_id(), None);
        for id in [
            "".to_owned(),
            "o".repeat(129),
            "other org".into(),
            "org/other".into(),
        ] {
            assert_eq!(
                WbsAssignmentContext::new(id, "row".into(), None),
                Err(WbsCandidateError::InvalidInput)
            );
        }
        for (generation, limit) in [(0, 1), (1, 0), (1, 11)] {
            assert_eq!(
                WbsCandidateQuery::new(context("row")?, generation, limit),
                Err(WbsCandidateError::InvalidInput)
            );
        }
        let boundary_query = WbsCandidateQuery::new(context("row")?, 1, 10)?;
        assert_eq!(boundary_query.context(), &context("row")?);
        assert_eq!(boundary_query.generation(), 1);
        assert_eq!(boundary_query.limit(), 10);
        for hash in ["a".repeat(63), "a".repeat(65), "z".repeat(64)] {
            assert_eq!(
                WbsCandidateSnapshotRef::new("board".into(), "v1".into(), hash),
                Err(WbsCandidateError::InvalidResponse)
            );
        }
        let boundary_sources = vec!["s".repeat(256); 16];
        let boundary_provenance = WbsCandidateProvenance::new(true, boundary_sources.clone())?;
        assert!(boundary_provenance.synthetic());
        assert_eq!(boundary_provenance.sources(), boundary_sources);
        assert_eq!(
            WbsCandidateProvenance::new(true, vec!["s".into(); 17]),
            Err(WbsCandidateError::InvalidResponse)
        );
        assert_eq!(
            WbsCandidateProvenance::new(true, vec!["s".repeat(257)]),
            Err(WbsCandidateError::InvalidResponse)
        );
        let error = candidate(0, "candidate-a", "user-a", true)
            .expect_err("rank zero must be an invalid candidate response");
        assert_eq!(
            error.downcast_ref::<WbsCandidateError>(),
            Some(&WbsCandidateError::InvalidResponse)
        );
        Ok(())
    }

    #[test]
    fn debug_of_every_candidate_component_redacts_user_and_source_data() -> TestResult {
        let query = query()?;
        let candidate = candidate(1, "candidate-private", "user-private", true)?;
        let snapshot = snapshot()?;
        let provenance = provenance()?;
        let output = format!(
            "{:?} {query:?} {:?} {snapshot:?} {:?} {:?} {candidate:?} {provenance:?}",
            query.context(),
            snapshot.reference(),
            candidate.identity(),
            candidate.algorithm()
        );
        let board = WbsAssignmentCandidates::ready(query, snapshot, provenance, vec![candidate])?;
        let output = format!("{output} {board:?}");
        for private in [
            "org-fixture",
            "row-a",
            "board-fixture",
            "approval-a",
            "ranker-fixture",
            "candidate-private",
            "user-private",
            "Private Candidate",
            "skill_match",
            "fixture-source",
        ] {
            assert!(!output.contains(private));
        }
        assert!(output.contains("REDACTED"));
        Ok(())
    }

    fn local_revision() -> TestResult<WbsLocalInputRevision> {
        Ok(WbsLocalInputRevision::new(
            "document-a".into(),
            "input-v1".into(),
            "a".repeat(64),
            "roster-v1".into(),
            "b".repeat(64),
        )?)
    }

    fn local_query() -> TestResult<WbsCandidateQuery> {
        Ok(WbsCandidateQuery::new(
            WbsAssignmentContext::new_local(local_revision()?, "row-a".into())?,
            1,
            10,
        )?)
    }

    fn local_snapshot() -> TestResult<WbsLocalResultSnapshot> {
        Ok(WbsLocalResultSnapshot::new(
            reference()?,
            local_revision()?,
            WbsLocalModelProvenance::new(
                "c".repeat(64),
                "provider-fixture".into(),
                "model-fixture".into(),
                "prompt-v1".into(),
            )?,
        )?)
    }

    fn local_board() -> TestResult<WbsAssignmentCandidates> {
        Ok(WbsAssignmentCandidates::ready_local(
            local_query()?,
            local_snapshot()?,
            provenance()?,
            vec![candidate(1, "candidate-a", "user-a", true)?],
        )?)
    }

    #[test]
    fn local_result_selects_without_inventing_server_identity() -> TestResult {
        let board = local_board()?;
        assert_eq!(board.query().context().organization_id(), None);
        assert_eq!(board.query().context().expected_wbs_version_id(), None);
        assert_eq!(
            board.query().context().local_input(),
            Some(&local_revision()?)
        );
        assert_eq!(board.snapshot(), None);
        assert_eq!(board.local_snapshot(), Some(&local_snapshot()?));
        assert_eq!(
            board.source().map(|s| s.kind()),
            Some(WbsCandidateSourceKind::LocalDocumentRoster)
        );
        assert_eq!(board.source_reference(), Some(&reference()?));
        assert_eq!(board.state(), WbsCandidateState::Ready);
        let selected = board.select(&local_query()?, &reference()?, "candidate-a")?;
        assert_eq!(selected.identity().user_id(), "user-a");
        assert_eq!(selected.display_name(), "Private Candidate");
        Ok(())
    }

    #[test]
    fn source_modes_cannot_cross_even_when_row_and_result_ids_match() -> TestResult {
        assert_eq!(
            WbsAssignmentCandidates::ready(local_query()?, snapshot()?, provenance()?, vec![],),
            Err(WbsCandidateError::InvalidInput)
        );
        assert_eq!(
            WbsAssignmentCandidates::ready_local(
                query()?,
                local_snapshot()?,
                provenance()?,
                vec![],
            ),
            Err(WbsCandidateError::InvalidInput)
        );
        let board = local_board()?;
        assert_eq!(
            board.select(&query()?, &reference()?, "candidate-a"),
            Err(WbsCandidateError::Stale)
        );
        let empty = WbsAssignmentCandidates::empty(local_query()?, provenance()?);
        assert_eq!(empty.state(), WbsCandidateState::Empty);
        assert_eq!(empty.source(), None);
        assert_eq!(
            empty.select(&local_query()?, &reference()?, "candidate-a"),
            Err(WbsCandidateError::NoCandidates)
        );
        Ok(())
    }

    #[test]
    fn local_selection_rejects_changed_registration_revisions_row_generation_and_result(
    ) -> TestResult {
        let board = local_board()?;
        let changes = [
            ("other-document", "input-v1", "a", "roster-v1", "b"),
            ("document-a", "input-v2", "a", "roster-v1", "b"),
            ("document-a", "input-v1", "d", "roster-v1", "b"),
            ("document-a", "input-v1", "a", "roster-v2", "b"),
            ("document-a", "input-v1", "a", "roster-v1", "e"),
        ];
        for (document, input_revision, input_hash, roster_revision, roster_hash) in changes {
            let revision = WbsLocalInputRevision::new(
                document.into(),
                input_revision.into(),
                input_hash.repeat(64),
                roster_revision.into(),
                roster_hash.repeat(64),
            )?;
            let changed_query = WbsCandidateQuery::new(
                WbsAssignmentContext::new_local(revision, "row-a".into())?,
                1,
                10,
            )?;
            assert_eq!(
                board.select(&changed_query, &reference()?, "candidate-a"),
                Err(WbsCandidateError::Stale)
            );
            assert_eq!(
                WbsAssignmentCandidates::ready_local(
                    changed_query,
                    local_snapshot()?,
                    provenance()?,
                    vec![]
                ),
                Err(WbsCandidateError::Stale)
            );
        }
        for changed_query in [
            WbsCandidateQuery::new(
                WbsAssignmentContext::new_local(local_revision()?, "row-b".into())?,
                1,
                10,
            )?,
            WbsCandidateQuery::new(local_query()?.context().clone(), 2, 10)?,
        ] {
            assert_eq!(
                board.select(&changed_query, &reference()?, "candidate-a"),
                Err(WbsCandidateError::Stale)
            );
        }
        let changed_result =
            WbsCandidateSnapshotRef::new("board-fixture".into(), "v1".into(), "f".repeat(64))?;
        assert_eq!(
            board.select(&local_query()?, &changed_result, "candidate-a"),
            Err(WbsCandidateError::Stale)
        );
        Ok(())
    }

    struct ServerRevalidationProbe {
        current: WbsAssignmentCandidates,
        calls: AtomicUsize,
    }
    #[async_trait::async_trait]
    impl WbsAssignmentCandidatesPort for ServerRevalidationProbe {
        async fn recommend_assignees(
            &self,
            _query: &WbsCandidateQuery,
        ) -> Result<WbsAssignmentCandidates, WbsCandidateError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.current.clone())
        }
    }

    #[tokio::test]
    async fn default_revalidation_refetches_only_server_and_rejects_changed_result() -> TestResult {
        let original = WbsAssignmentCandidates::ready(
            query()?,
            snapshot()?,
            provenance()?,
            vec![candidate(1, "candidate-a", "user-a", true)?],
        )?;
        let provider = ServerRevalidationProbe {
            current: original.clone(),
            calls: AtomicUsize::new(0),
        };
        assert_eq!(provider.revalidate_assignees(&original).await, Ok(()));
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        let changed = ServerRevalidationProbe {
            current: WbsAssignmentCandidates::empty(query()?, provenance()?),
            calls: AtomicUsize::new(0),
        };
        assert_eq!(
            changed.revalidate_assignees(&original).await,
            Err(WbsCandidateError::Stale)
        );
        assert_eq!(changed.calls.load(Ordering::SeqCst), 1);
        Ok(())
    }

    #[tokio::test]
    async fn default_local_revalidation_never_invokes_the_model_provider() -> TestResult {
        let original = local_board()?;
        let provider = ServerRevalidationProbe {
            current: original.clone(),
            calls: AtomicUsize::new(0),
        };
        assert_eq!(
            provider.revalidate_assignees(&original).await,
            Err(WbsCandidateError::UnsupportedContract)
        );
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
        Ok(())
    }
}
