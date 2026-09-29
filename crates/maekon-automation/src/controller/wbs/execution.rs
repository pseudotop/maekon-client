//! Audited recommendation reads bound to one authenticated document session.
//! Native writes and execution-ticket consumption are connected separately.

use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use maekon_core::models::audit::{AuditLevel, AuditStatus};
use maekon_core::models::automation::wbs::*;
use maekon_core::models::wbs_assignment_candidates::{
    local::WbsLocalInputRevision, WbsAssignmentCandidates, WbsCandidateQuery, WbsCandidateSource,
    WbsCandidateSourceKind,
};
use maekon_core::models::wbs_cell_apply::{WbsCellTargetError, WbsCellTargetSnapshot};
use maekon_core::ports::wbs_assignment_candidates::WbsAssignmentCandidatesPort;
use maekon_core::ports::wbs_cell_target::WbsCellTargetPort;

use super::{document_session_id, WbsDocumentSessionAccess, WbsDocumentSessions};

const IO_TIMEOUT: Duration = Duration::from_secs(7);
const LOCAL_MODEL_TIMEOUT: Duration = Duration::from_secs(60);

/// Trusted composition binds both raw ports to the document registry's scope.
/// Clones share the same session reservation and retained recommendation.
#[derive(Clone)]
pub struct WbsAutomationService {
    runtime: Arc<RecommendationRuntime>,
}

struct RecommendationRuntime {
    documents: WbsDocumentSessions,
    target: Arc<dyn WbsCellTargetPort>,
    candidates: Arc<dyn WbsAssignmentCandidatesPort>,
}

pub(super) struct Recommendation {
    target: WbsCellTargetSnapshot,
    board: WbsAssignmentCandidates,
}

struct RecommendationWork {
    runtime: Arc<RecommendationRuntime>,
    access: WbsDocumentSessionAccess,
    generation: u64,
}

impl WbsAutomationService {
    pub fn new(
        documents: WbsDocumentSessions,
        target: Arc<dyn WbsCellTargetPort>,
        candidates: Arc<dyn WbsAssignmentCandidatesPort>,
    ) -> Self {
        Self {
            runtime: Arc::new(RecommendationRuntime {
                documents,
                target,
                candidates,
            }),
        }
    }

    pub async fn offer_document_consent(&self) -> Result<WbsConsentOffer, WbsAutomationError> {
        self.runtime.documents.offer_document_consent().await
    }

    pub async fn open_session(
        &self,
        acceptance: WbsConsentAcceptance,
    ) -> Result<WbsSessionView, WbsAutomationError> {
        self.runtime.documents.open_session(acceptance).await
    }

    pub async fn recommend(
        &self,
        auth: WbsSessionAuthorization,
    ) -> Result<WbsRecommendationView, WbsAutomationError> {
        let handle =
            tokio::runtime::Handle::try_current().map_err(|_| WbsAutomationError::Unavailable)?;
        let work = self.runtime.reserve(&auth)?;
        // Dropping the IPC future detaches its JoinHandle, not the retained worker.
        handle
            .spawn(work.run())
            .await
            .map_err(|_| WbsAutomationError::Unavailable)?
    }

    /// Retrieve the last audited view after a detached read finishes. This does
    /// not reobserve the workbook or grant confirmation/execution authority.
    pub fn read_recommendation(
        &self,
        auth: &WbsSessionAuthorization,
    ) -> Result<WbsRecommendationView, WbsAutomationError> {
        let rt = &self.runtime.documents.runtime;
        let state = rt.state()?;
        let session = rt.authenticate(&state, auth)?;
        let access = WbsDocumentSessionAccess {
            view: session.view.clone(),
            pin: session.pin.clone(),
            cancelled: session.cancelled.clone(),
        };
        access.revalidate()?;
        let recommendation = session
            .recommendation
            .as_ref()
            .ok_or(WbsAutomationError::StaleCandidate)?;
        self.runtime.render(&access, recommendation)
    }
}

impl RecommendationRuntime {
    fn reserve(
        self: &Arc<Self>,
        auth: &WbsSessionAuthorization,
    ) -> Result<RecommendationWork, WbsAutomationError> {
        let rt = &self.documents.runtime;
        let mut state = rt.state()?;
        let session = rt.authenticate(&state, auth)?;
        let access = WbsDocumentSessionAccess {
            view: session.view.clone(),
            pin: session.pin.clone(),
            cancelled: session.cancelled.clone(),
        };
        access.revalidate()?;
        if session.busy {
            return Err(WbsAutomationError::Busy);
        }
        let generation = state
            .generation
            .checked_add(1)
            .ok_or(WbsAutomationError::CapacityExceeded)?;
        let session = state
            .sessions
            .get_mut(auth.session_id().as_str())
            .ok_or(WbsAutomationError::InvalidCapability)?;
        session.busy = true;
        session.generation = generation;
        session.recommendation = None;
        state.generation = generation;
        Ok(RecommendationWork {
            runtime: self.clone(),
            access,
            generation,
        })
    }

    async fn audit(
        &self,
        access: &WbsDocumentSessionAccess,
        operation: &WbsIdentifier,
        status: AuditStatus,
    ) -> Result<(), WbsAutomationError> {
        let mut audit = tokio::time::timeout(
            IO_TIMEOUT,
            self.documents.runtime.controller.audit_logger.write(),
        )
        .await
        .map_err(|_| WbsAutomationError::AuditUnavailable)?;
        if !audit.has_persistence() {
            return Err(WbsAutomationError::AuditUnavailable);
        }
        audit
            .try_log_with_status_and_time(
                AuditLevel::Full,
                operation.as_str(),
                access.view().authorization().session_id().as_str(),
                "wbs_assignee_context_read",
                status,
                "wbs_scoped_read",
                0,
            )
            .map_err(|_| WbsAutomationError::AuditUnavailable)
    }

    async fn fetch(
        &self,
        access: &WbsDocumentSessionAccess,
        query: &WbsCandidateQuery,
    ) -> Result<WbsAssignmentCandidates, WbsAutomationError> {
        access.revalidate()?;
        let bound = match query.context().source_kind() {
            WbsCandidateSourceKind::ServerBoard => IO_TIMEOUT,
            WbsCandidateSourceKind::LocalDocumentRoster => LOCAL_MODEL_TIMEOUT,
        };
        let until = access.pin.until.min(Instant::now() + bound);
        let request = self.candidates.recommend_assignees(query);
        tokio::pin!(request);
        let deadline = tokio::time::sleep_until(until.into());
        tokio::pin!(deadline);
        let mut checks = tokio::time::interval(Duration::from_millis(50));
        let board = loop {
            tokio::select! {
                result = &mut request => break result.map_err(|_| WbsAutomationError::StaleCandidate)?,
                _ = &mut deadline => return Err(WbsAutomationError::Unavailable),
                _ = checks.tick() => access.revalidate()?,
            }
        };
        if Instant::now() >= until {
            return Err(WbsAutomationError::Unavailable);
        }
        access.revalidate()?;
        if board.query() != query {
            return Err(WbsAutomationError::StaleCandidate);
        }
        Ok(board)
    }

    fn render(
        &self,
        access: &WbsDocumentSessionAccess,
        recommendation: &Recommendation,
    ) -> Result<WbsRecommendationView, WbsAutomationError> {
        let target = &recommendation.target;
        let board = &recommendation.board;
        let query = board.query();
        let context = match query.context().source_kind() {
            WbsCandidateSourceKind::ServerBoard => WbsContextView::new(
                id(query
                    .context()
                    .organization_id()
                    .ok_or(WbsAutomationError::InvalidInput)?)?,
                id(query.context().wbs_item_id())?,
                query
                    .context()
                    .expected_wbs_version_id()
                    .map(id)
                    .transpose()?,
            ),
            WbsCandidateSourceKind::LocalDocumentRoster => WbsContextView::new_local(
                id(query.context().wbs_item_id())?,
                local_context(
                    query
                        .context()
                        .local_input()
                        .ok_or(WbsAutomationError::InvalidInput)?,
                )?,
            ),
        };
        let source = board
            .source()
            .map(|source| -> Result<_, WbsAutomationError> {
                Ok(match source {
                    WbsCandidateSource::ServerBoard(snapshot) => WbsSourceView::new(
                        id(snapshot.reference().snapshot_id())?,
                        id(snapshot.reference().snapshot_version())?,
                        WbsProof::new(snapshot.reference().snapshot_hash().into())?,
                        id(snapshot.wbs_version_id())?,
                        WbsProof::new(snapshot.wbs_content_hash().into())?,
                        id(snapshot.approval_id())?,
                    ),
                    WbsCandidateSource::LocalDocumentRoster(snapshot) => WbsSourceView::new_local(
                        id(snapshot.reference().snapshot_id())?,
                        id(snapshot.reference().snapshot_version())?,
                        WbsProof::new(snapshot.reference().snapshot_hash().into())?,
                        WbsLocalSourceView::new(
                            local_context(snapshot.input())?,
                            WbsProof::new(snapshot.model().provider_selection_digest().into())?,
                            text(snapshot.model().provider_name())?,
                            text(snapshot.model().model())?,
                            id(snapshot.model().prompt_revision())?,
                        ),
                    ),
                })
            })
            .transpose()?;
        let cards = board
            .candidates()
            .iter()
            .map(|candidate| -> Result<_, WbsAutomationError> {
                Ok(WbsCandidateCard::new(
                    id(candidate.identity().candidate_id())?,
                    text(candidate.display_name())?,
                    candidate.rank(),
                    candidate.eligible(),
                    text(candidate.rank_reason())?,
                    self.documents.runtime.signer.candidate_proof(
                        access.view().authorization(),
                        target,
                        board,
                        candidate,
                    )?,
                ))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let provenance = board
            .provenance()
            .sources()
            .iter()
            .map(|value| text(value))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(WbsRecommendationView::new(
            access.view(),
            WbsGeneration::new(query.generation())?,
            context,
            source,
            (board.provenance().synthetic(), provenance),
            WbsTargetHint::new(target.identity().row(), target.identity().column()),
            cards,
        )?
        .with_anchor(target.anchor()))
    }
}

// A ready native future can finish without yielding, so timeout_at alone does
// not reject an already expired or overrun read. Bound acceptance on both sides.
pub(super) async fn native_read<T>(
    until: Instant,
    request: impl Future<Output = Result<T, WbsCellTargetError>>,
) -> Result<T, WbsAutomationError> {
    if Instant::now() >= until {
        return Err(WbsAutomationError::StaleSelection);
    }
    let result = tokio::time::timeout_at(until.into(), request)
        .await
        .map_err(|_| WbsAutomationError::StaleSelection)?
        .map_err(|_| WbsAutomationError::StaleSelection)?;
    if Instant::now() >= until {
        return Err(WbsAutomationError::StaleSelection);
    }
    Ok(result)
}

impl RecommendationWork {
    async fn compute(&self) -> Result<(Recommendation, WbsRecommendationView), WbsAutomationError> {
        let rt = &self.runtime;
        self.access.revalidate()?;
        let target = native_read(
            self.access.pin.until.min(Instant::now() + IO_TIMEOUT),
            rt.target.capture_selection(),
        )
        .await?;
        self.access.revalidate()?;
        if !rt
            .documents
            .runtime
            .scope
            .allowed_contexts()
            .contains(target.identity().context())
        {
            return Err(WbsAutomationError::StaleSelection);
        }
        let query =
            WbsCandidateQuery::new(target.identity().context().clone(), self.generation, 10)
                .map_err(|_| WbsAutomationError::InvalidInput)?;
        let board = rt.fetch(&self.access, &query).await?;
        self.access.revalidate()?;
        native_read(
            self.access.pin.until.min(Instant::now() + IO_TIMEOUT),
            rt.target.validate_bound_cell(&target),
        )
        .await?;
        self.access.revalidate()?;
        let recommendation = Recommendation { target, board };
        let view = rt.render(&self.access, &recommendation)?;
        Ok((recommendation, view))
    }

    async fn run(self) -> Result<WbsRecommendationView, WbsAutomationError> {
        let operation = document_session_id()?;
        self.runtime
            .audit(&self.access, &operation, AuditStatus::Started)
            .await?;
        let result = self.compute().await;
        self.runtime
            .audit(
                &self.access,
                &operation,
                if result.is_ok() {
                    AuditStatus::Completed
                } else {
                    AuditStatus::Failed
                },
            )
            .await?;
        let (recommendation, view) = result?;
        let mut state = self.runtime.documents.runtime.state()?;
        self.access.revalidate()?;
        let session = state
            .sessions
            .get_mut(self.access.view().authorization().session_id().as_str())
            .ok_or(WbsAutomationError::InvalidCapability)?;
        if !session.busy || session.generation != self.generation {
            return Err(WbsAutomationError::StaleCandidate);
        }
        session.recommendation = Some(recommendation);
        session.busy = false;
        Ok(view)
    }
}

impl Drop for RecommendationWork {
    fn drop(&mut self) {
        if let Ok(mut state) = self.runtime.documents.runtime.state() {
            if let Some(session) = state
                .sessions
                .get_mut(self.access.view().authorization().session_id().as_str())
            {
                if session.generation == self.generation {
                    session.busy = false;
                }
            }
        }
    }
}

fn id(value: &str) -> Result<WbsIdentifier, WbsAutomationError> {
    WbsIdentifier::new(value.into())
}
fn text(value: &str) -> Result<WbsDisplayText, WbsAutomationError> {
    WbsDisplayText::new(value.into())
}
fn local_context(input: &WbsLocalInputRevision) -> Result<WbsLocalContextView, WbsAutomationError> {
    Ok(WbsLocalContextView::new(
        id(input.document_registration_id())?,
        id(input.input_revision())?,
        WbsProof::new(input.input_hash().into())?,
        id(input.roster_revision())?,
        WbsProof::new(input.roster_hash().into())?,
    ))
}
