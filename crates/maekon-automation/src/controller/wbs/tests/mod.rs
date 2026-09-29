use chrono::{DateTime, Utc};
use maekon_core::models::automation::wbs::*;
use maekon_core::models::wbs_assignment_candidates::local::*;
use maekon_core::models::wbs_assignment_candidates::*;
use maekon_core::models::wbs_cell_apply::*;
use zeroize::Zeroizing;

use super::WbsProofSigner;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

struct ProofFixture {
    auth: WbsSessionAuthorization,
    target: WbsCellTargetSnapshot,
    board: WbsAssignmentCandidates,
    candidate: WbsAssignmentCandidate,
}

fn fixture(local: bool, changed: &str) -> TestResult<ProofFixture> {
    let text = |field: &str, original: &str| {
        if field == changed {
            format!("{original}-v2")
        } else {
            original.into()
        }
    };
    let hash = |field: &str, original: u8| {
        char::from(original + u8::from(field == changed))
            .to_string()
            .repeat(64)
    };
    let number = |field: &str, original: u64| original + u64::from(field == changed);
    let input = WbsLocalInputRevision::new(
        text("document", "document-1"),
        text("input_revision", "input-v1"),
        hash("input_hash", b'1'),
        text("roster_revision", "roster-v1"),
        hash("roster_hash", b'2'),
    )?;
    let context = if local {
        WbsAssignmentContext::new_local(input.clone(), text("item", "item-1"))?
    } else {
        WbsAssignmentContext::new(
            text("organization", "org-1"),
            text("item", "item-1"),
            (changed != "expected_version_absent").then(|| text("wbs_version", "wbs-v1")),
        )?
    };
    let auth = WbsSessionAuthorization::new(
        WbsIdentifier::new(text("session", "session-1"))?,
        WbsProof::new(hash("capability", b'd'))?,
    );
    let target = WbsCellTargetSnapshot::new(
        WbsCellBinding::new(
            text("binding", "binding-1"),
            number("binding_generation", 1),
        )?,
        WbsCellIdentity::new(
            u32::try_from(number("process", 42))?,
            number("process_start", 123),
            text("workbook", "book-1"),
            text("worksheet", "sheet-1"),
            u32::try_from(number("row", 5))?,
            u32::try_from(number("column", 5))?,
            context.clone(),
        )?,
        text("before", "Before"),
        None,
    )?;
    let reference = WbsCandidateSnapshotRef::new(
        text("snapshot_id", "snapshot-1"),
        text("snapshot_version", "v1"),
        hash("snapshot_hash", b'a'),
    )?;
    let as_of = DateTime::parse_from_rfc3339("2026-09-01T12:00:00Z")?.with_timezone(&Utc)
        + chrono::Duration::seconds(i64::from(changed == "algorithm_as_of"));
    let candidate = WbsAssignmentCandidate::new(
        WbsCandidateIdentity::new(
            text("candidate_id", "candidate-1"),
            hash("candidate_hash", b'c'),
            text("user_id", "user-1"),
        )?,
        text("display_name", "Synthetic assignee"),
        u32::try_from(number("rank", 1))?,
        changed != "eligible",
        text("reason", "Synthetic skill fit"),
        reference.clone(),
        WbsCandidateAlgorithm::new(
            text("algorithm", "ranker"),
            text("algorithm_version", "v1"),
            as_of,
        )?,
    )?;
    let query = WbsCandidateQuery::new(
        context,
        number("query_generation", 1),
        if changed == "query_limit" { 9 } else { 10 },
    )?;
    let provenance = WbsCandidateProvenance::new(
        changed != "synthetic",
        vec![text("provenance", "synthetic-fixture")],
    )?;
    let board = if local {
        WbsAssignmentCandidates::ready_local(
            query,
            WbsLocalResultSnapshot::new(
                reference,
                input,
                WbsLocalModelProvenance::new(
                    hash("provider_selection", b'e'),
                    text("provider_name", "provider-fixture"),
                    text("model", "model-fixture"),
                    text("prompt_revision", "prompt-v1"),
                )?,
            )?,
            provenance,
            vec![candidate.clone()],
        )?
    } else {
        WbsAssignmentCandidates::ready(
            query,
            WbsCandidateSnapshot::new(
                reference,
                text("wbs_version", "wbs-v1"),
                hash("wbs_hash", b'b'),
                text("approval", "approval-1"),
            )?,
            provenance,
            vec![candidate.clone()],
        )?
    };
    Ok(ProofFixture {
        auth,
        target,
        board,
        candidate,
    })
}

fn signer(byte: u8) -> Result<WbsProofSigner, WbsAutomationError> {
    WbsProofSigner::new(Zeroizing::new(vec![byte; 32]))
}

fn prove(signer: &WbsProofSigner, input: &ProofFixture) -> Result<WbsProof, WbsAutomationError> {
    signer.candidate_proof(&input.auth, &input.target, &input.board, &input.candidate)
}

fn verify(
    signer: &WbsProofSigner,
    input: &ProofFixture,
    proof: &WbsProof,
) -> Result<(), WbsAutomationError> {
    signer.verify_candidate(
        &input.auth,
        &input.target,
        &input.board,
        &input.candidate,
        proof,
    )
}

fn proof_time(value: &str) -> TestResult<DateTime<Utc>> {
    Ok(DateTime::parse_from_rfc3339(value)?.with_timezone(&Utc))
}

fn proof_consent(changed: &str) -> TestResult<maekon_core::consent::ConsentRecord> {
    use maekon_core::consent::{ConsentPermissions, ConsentRecord};
    let granted = proof_time("2026-09-01T12:00:00Z")?;
    Ok(ConsentRecord {
        consent_id: if changed == "consent_id" {
            "consent-2"
        } else {
            "consent-1"
        }
        .into(),
        version: if changed == "policy" {
            "2026-10"
        } else {
            "2026-09"
        }
        .into(),
        granted_at: granted + chrono::Duration::seconds(i64::from(changed == "granted")),
        expires_at: if changed == "no_expiry" {
            None
        } else {
            Some(
                proof_time("2026-10-01T12:00:00Z")?
                    + chrono::Duration::seconds(i64::from(changed == "consent_expiry")),
            )
        },
        revoked_at: (changed == "revoked").then_some(granted),
        data_deletion_requested: changed == "deletion",
        erasure_nonce: match changed {
            "erasure" => Some("erasure-1".into()),
            "empty_erasure" => Some(String::new()),
            _ => None,
        },
        permissions: ConsentPermissions {
            process_monitoring: changed != "process_monitoring",
            ..ConsentPermissions::default()
        },
        data_retention_days: 30,
    })
}

fn proof_scope(input: &ProofFixture, changed: &str) -> TestResult<WbsDocumentScope> {
    let context = input.target.identity().context();
    let local = context.source_kind() == WbsCandidateSourceKind::LocalDocumentRoster;
    let mut contexts = vec![context.clone()];
    if changed == "extra_context" {
        contexts.push(if local {
            WbsAssignmentContext::new_local(
                context.local_input().ok_or("local input")?.clone(),
                "item-2".into(),
            )?
        } else {
            WbsAssignmentContext::new("org-1".into(), "item-2".into(), Some("wbs-v1".into()))?
        });
    }
    let scope_id = WbsIdentifier::new(
        if changed == "scope" {
            "scope-2"
        } else {
            "scope-1"
        }
        .into(),
    )?;
    let registration = WbsProof::new(if changed == "registration" { "7" } else { "8" }.repeat(64))?;
    let label = WbsDisplayText::new(
        if changed == "label" {
            "Different workbook"
        } else {
            "Synthetic workbook"
        }
        .into(),
    )?;
    Ok(if local {
        WbsDocumentScope::new_local(scope_id, registration, label, contexts)?
    } else {
        WbsDocumentScope::new(
            scope_id,
            registration,
            WbsProof::new(if changed == "origin" { "7" } else { "9" }.repeat(64))?,
            label,
            contexts,
        )?
    })
}

fn proof_acceptance(
    signer: &WbsProofSigner,
    scope: &WbsDocumentScope,
) -> TestResult<WbsConsentAcceptance> {
    let offer = WbsIdentifier::new("offer-1".into())?;
    let nonce = signer.document_consent_nonce(
        scope,
        &proof_consent("")?,
        &offer,
        &proof_time("2026-09-01T12:01:00Z")?,
    )?;
    Ok(WbsConsentAcceptance::new(
        offer,
        nonce,
        scope.scope_id().clone(),
        WbsIdentifier::new(WBS_CONSENT_NOTICE.into())?,
    ))
}

struct ConsentProbe {
    on_read: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
    record: Option<maekon_core::consent::ConsentRecord>,
    status: maekon_core::consent::ConsentStatus,
    effective: bool,
    pending: bool,
    erasure: Option<String>,
    deletion: std::sync::Arc<std::sync::atomic::AtomicBool>,
    erasing: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
impl ConsentProbe {
    fn valid() -> TestResult<Self> {
        let mut record = proof_consent("")?;
        record.expires_at = Some(Utc::now() + chrono::Duration::minutes(60));
        Ok(Self {
            record: Some(record),
            on_read: None,
            status: maekon_core::consent::ConsentStatus::Valid,
            effective: true,
            pending: false,
            erasure: None,
            deletion: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            erasing: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        })
    }
}
impl maekon_core::ports::consent_manager::ConsentManagerPort for ConsentProbe {
    fn check_consent(&self) -> maekon_core::consent::ConsentStatus {
        self.status.clone()
    }
    fn current_consent(&self) -> Option<maekon_core::consent::ConsentRecord> {
        if let Some(on_read) = &self.on_read {
            on_read();
        }
        self.record.clone()
    }
    fn effective_permissions(&self) -> maekon_core::consent::ConsentPermissions {
        maekon_core::consent::ConsentPermissions {
            process_monitoring: self.effective,
            ..Default::default()
        }
    }
    fn status_and_permissions(
        &self,
    ) -> (
        maekon_core::consent::ConsentStatus,
        maekon_core::consent::ConsentPermissions,
    ) {
        panic!("A gate must not use raw UI permissions")
    }
    fn grant_consent(
        &self,
        _: maekon_core::consent::ConsentPermissions,
        _: u32,
    ) -> Result<(), maekon_core::error::CoreError> {
        panic!("Read-only observation cannot grant consent")
    }
    fn revoke_consent(&self) -> Result<(), maekon_core::error::CoreError> {
        panic!("Read-only observation cannot revoke consent")
    }
    fn has_pending_deletion(&self) -> bool {
        self.pending
    }
    fn pending_erasure_id(&self) -> Option<String> {
        self.erasure.clone()
    }
    fn clear_pending_deletion(&self) {
        panic!("Read-only observation cannot clear erasure")
    }
    fn deletion_flag(&self) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
        self.deletion.clone()
    }
    fn erasing(&self) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
        self.erasing.clone()
    }
}

mod authorization;
mod candidates;
mod execution;

struct AuthorityFixture {
    _directory: tempfile::TempDir,
    config: maekon_core::config_manager::ConfigManager,
    authority: super::WbsLiveAuthority,
    controller: std::sync::Arc<super::super::AutomationController>,
    scope: WbsDocumentScope,
}

fn authority_fixture(
    consent: std::sync::Arc<dyn maekon_core::ports::consent_manager::ConsentManagerPort>,
    controller_change: &str,
    local: bool,
) -> TestResult<AuthorityFixture> {
    use crate::{audit::AuditLogger, policy::PolicyClient, sandbox::NoOpSandbox};
    use maekon_core::config::{ConfirmationRequirement, SandboxConfig};
    use std::sync::Arc;
    let directory = tempfile::tempdir()?;
    let config = maekon_core::config_manager::ConfigManager::with_paths(
        directory.path().join("config.json"),
        None,
    )?;
    let mut current = (*config.snapshot()).clone();
    current.automation.enabled = true;
    current.automation.sandbox.enabled = false;
    current.automation.confirmation_policy = ConfirmationRequirement::Auto;
    config.update(current)?;
    let mut controller = super::super::AutomationController::new(
        Arc::new(PolicyClient::new()),
        Arc::new(tokio::sync::RwLock::new(AuditLogger::new(16, 16))),
        Arc::new(NoOpSandbox),
        SandboxConfig {
            enabled: controller_change == "sandbox",
            ..Default::default()
        },
    );
    controller.set_enabled(controller_change != "disabled");
    controller.set_confirmation_policy(if controller_change == "block" {
        ConfirmationRequirement::Block
    } else {
        ConfirmationRequirement::Auto
    });
    let original = proof_scope(&fixture(local, "")?, "")?;
    let scope = if local {
        original
    } else {
        WbsDocumentScope::new(
            original.scope_id().clone(),
            original.native_registration_digest().clone(),
            super::wbs_server_origin_digest(&config.snapshot())?,
            original.document_display_label().clone(),
            original.allowed_contexts().to_vec(),
        )?
    };
    let controller = Arc::new(controller);
    Ok(AuthorityFixture {
        _directory: directory,
        config: config.clone(),
        authority: super::WbsLiveAuthority::new(controller.clone(), config, consent, scope.clone()),
        controller,
        scope,
    })
}

fn accept_offer(offer: &WbsConsentOffer) -> WbsConsentAcceptance {
    WbsConsentAcceptance::new(
        offer.offer_id().clone(),
        offer.nonce().clone(),
        offer.scope_id().clone(),
        offer.notice_version().clone(),
    )
}

async fn document_sessions_fixture(
    audit: bool,
) -> TestResult<(AuthorityFixture, super::WbsDocumentSessions)> {
    use std::sync::Arc;
    let consent = Arc::new(ConsentProbe::valid()?);
    let fixture = authority_fixture(consent.clone(), "", true)?;
    if audit {
        *fixture.controller.audit_logger.write().await = session_audit();
    }
    let sessions = super::WbsDocumentSessions::new(
        fixture.controller.clone(),
        fixture.config.clone(),
        consent,
        fixture.scope.clone(),
        Zeroizing::new(vec![9; 32]),
    )?;
    Ok((fixture, sessions))
}

fn session_audit() -> crate::audit::AuditLogger {
    crate::audit::AuditLogger::new(16, 16).with_persistence(std::sync::Arc::new(
        |_: &maekon_core::models::audit::AuditEntry| {
            panic!("Session establishment must not report a native audit event")
        },
    ))
}

fn assert_session_pending<F: std::future::Future>(future: std::pin::Pin<&mut F>) {
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    assert!(future.poll(&mut context).is_pending());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn document_sessions_consume_one_offer_once_and_authenticate_every_capability() -> TestResult
{
    let (_fixture, sessions) = document_sessions_fixture(true).await?;
    let offer = sessions.offer_document_consent().await?;
    assert_eq!(offer.permitted_operations(), &WBS_CONSENT_OPERATIONS);
    assert!(offer.expires_at() > &Utc::now());
    for field in ["offer", "nonce", "scope", "notice"] {
        let invalid = WbsConsentAcceptance::new(
            if field == "offer" {
                WbsIdentifier::new("unknown".into())?
            } else {
                offer.offer_id().clone()
            },
            if field == "nonce" {
                WbsProof::new("0".repeat(64))?
            } else {
                offer.nonce().clone()
            },
            if field == "scope" {
                WbsIdentifier::new("other-scope".into())?
            } else {
                offer.scope_id().clone()
            },
            if field == "notice" {
                WbsIdentifier::new("other-notice".into())?
            } else {
                offer.notice_version().clone()
            },
        );
        assert_eq!(
            sessions.open_session(invalid).await,
            Err(WbsAutomationError::InvalidProof),
            "{field}"
        );
    }
    let (_other_fixture, other) = document_sessions_fixture(true).await?;
    assert_eq!(
        other.open_session(accept_offer(&offer)).await,
        Err(WbsAutomationError::InvalidProof)
    );
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(3));
    let mut tasks = Vec::new();
    for _ in 0..2 {
        let sessions = sessions.clone();
        let acceptance = accept_offer(&offer);
        let barrier = barrier.clone();
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            sessions.open_session(acceptance).await
        }));
    }
    barrier.wait().await;
    let mut successes = Vec::new();
    let mut refusals = 0;
    for task in tasks {
        match task.await? {
            Ok(view) => successes.push(view),
            Err(error) => {
                assert_eq!(error, WbsAutomationError::InvalidProof);
                refusals += 1;
            }
        }
    }
    assert_eq!((successes.len(), refusals), (1, 1));
    let view = successes.pop().ok_or("missing session")?;
    assert_eq!(view.scope_id(), offer.scope_id());
    let auth = view.authorization();
    let access = sessions.authorize(auth)?;
    assert_eq!(access.view(), &view);
    assert_eq!(access.revalidate(), Ok(()));
    for invalid in [
        WbsSessionAuthorization::new(
            WbsIdentifier::new("unknown".into())?,
            auth.capability().clone(),
        ),
        WbsSessionAuthorization::new(auth.session_id().clone(), WbsProof::new("0".repeat(64))?),
    ] {
        assert_eq!(
            sessions.authorize(&invalid).err(),
            Some(WbsAutomationError::InvalidCapability)
        );
        assert_eq!(
            sessions.cancel(&invalid),
            Err(WbsAutomationError::InvalidCapability)
        );
        assert_eq!(access.revalidate(), Ok(()));
    }
    assert_eq!(
        other.authorize(auth).err(),
        Some(WbsAutomationError::InvalidCapability)
    );
    assert_eq!(
        sessions.open_session(accept_offer(&offer)).await,
        Err(WbsAutomationError::InvalidProof)
    );
    sessions.cancel(auth)?;
    sessions.cancel(auth)?;
    assert_eq!(access.revalidate(), Err(WbsAutomationError::Expired));
    assert_eq!(
        sessions.authorize(auth).err(),
        Some(WbsAutomationError::Expired)
    );
    Ok(())
}

#[tokio::test]
async fn document_sessions_bound_capacity_without_evicting_retained_or_consumed_records(
) -> TestResult {
    let (_fixture, sessions) = document_sessions_fixture(true).await?;
    let mut offers = Vec::new();
    for _ in 0..8 {
        offers.push(sessions.offer_document_consent().await?);
    }
    assert_eq!(
        sessions.offer_document_consent().await,
        Err(WbsAutomationError::CapacityExceeded)
    );
    let mut views = Vec::new();
    for offer in &offers {
        views.push(sessions.open_session(accept_offer(offer)).await?);
    }
    let pending = sessions.offer_document_consent().await?;
    assert_eq!(
        sessions.open_session(accept_offer(&pending)).await,
        Err(WbsAutomationError::CapacityExceeded)
    );
    let retained = sessions.authorize(views[0].authorization())?;
    for view in &views {
        sessions.cancel(view.authorization())?;
    }
    let replacement = sessions.open_session(accept_offer(&pending)).await?;
    {
        let state = sessions.runtime.state()?;
        assert_eq!(state.sessions.len(), 2);
        assert!(state
            .sessions
            .contains_key(views[0].authorization().session_id().as_str()));
        assert!(state
            .sessions
            .contains_key(replacement.authorization().session_id().as_str()));
    }
    assert_eq!(retained.revalidate(), Err(WbsAutomationError::Expired));
    sessions.cancel(views[0].authorization())?;
    assert_eq!(
        sessions.cancel(views[1].authorization()),
        Err(WbsAutomationError::InvalidCapability)
    );
    assert_eq!(
        sessions.open_session(accept_offer(&offers[0])).await,
        Err(WbsAutomationError::InvalidProof)
    );
    drop(retained);
    let fresh = sessions.offer_document_consent().await?;
    sessions.open_session(accept_offer(&fresh)).await?;
    assert_eq!(sessions.runtime.state()?.sessions.len(), 2);
    assert_eq!(
        sessions.cancel(views[0].authorization()),
        Err(WbsAutomationError::InvalidCapability)
    );
    assert_eq!(
        sessions.open_session(accept_offer(&pending)).await,
        Err(WbsAutomationError::InvalidProof)
    );
    assert_eq!(
        sessions
            .authorize(replacement.authorization())?
            .revalidate(),
        Ok(())
    );
    Ok(())
}

#[tokio::test]
async fn document_sessions_expire_offers_and_retain_authenticated_cancellation() -> TestResult {
    use std::time::{Duration, Instant};
    let (_fixture, sessions) = document_sessions_fixture(true).await?;
    let expired = sessions.offer_document_consent().await?;
    sessions
        .runtime
        .state()?
        .offers
        .get_mut(expired.offer_id().as_str())
        .ok_or("offer")?
        .pin
        .until = Instant::now() - Duration::from_secs(1);
    assert_eq!(
        sessions.open_session(accept_offer(&expired)).await,
        Err(WbsAutomationError::Expired)
    );
    let fresh = sessions.offer_document_consent().await?;
    assert_eq!(sessions.runtime.state()?.offers.len(), 1);
    assert_eq!(
        sessions.open_session(accept_offer(&expired)).await,
        Err(WbsAutomationError::InvalidProof)
    );
    let view = sessions.open_session(accept_offer(&fresh)).await?;
    sessions
        .runtime
        .state()?
        .sessions
        .get_mut(view.authorization().session_id().as_str())
        .ok_or("session")?
        .pin
        .until = Instant::now() - Duration::from_secs(1);
    assert_eq!(
        sessions.authorize(view.authorization()).err(),
        Some(WbsAutomationError::Expired)
    );
    sessions.cancel(view.authorization())?;
    let replacement = sessions.offer_document_consent().await?;
    sessions.open_session(accept_offer(&replacement)).await?;
    assert_eq!(sessions.runtime.state()?.sessions.len(), 1);
    assert_eq!(
        sessions.cancel(view.authorization()),
        Err(WbsAutomationError::InvalidCapability)
    );
    Ok(())
}

#[tokio::test]
async fn document_sessions_live_erasure_invalidates_access_but_allows_cancellation() -> TestResult {
    use std::sync::{atomic::Ordering, Arc};
    let consent = Arc::new(ConsentProbe::valid()?);
    let fixture = authority_fixture(consent.clone(), "", true)?;
    *fixture.controller.audit_logger.write().await = session_audit();
    let sessions = super::WbsDocumentSessions::new(
        fixture.controller.clone(),
        fixture.config.clone(),
        consent.clone(),
        fixture.scope.clone(),
        Zeroizing::new(vec![8; 32]),
    )?;
    let offer = sessions.offer_document_consent().await?;
    let view = sessions.open_session(accept_offer(&offer)).await?;
    let access = sessions.authorize(view.authorization())?;
    consent.deletion.store(true, Ordering::SeqCst);
    assert_eq!(
        access.revalidate(),
        Err(WbsAutomationError::ConsentRequired)
    );
    assert_eq!(
        sessions.authorize(view.authorization()).err(),
        Some(WbsAutomationError::ConsentRequired)
    );
    assert_eq!(
        sessions.offer_document_consent().await,
        Err(WbsAutomationError::ConsentRequired)
    );
    sessions.cancel(view.authorization())?;
    consent.deletion.store(false, Ordering::SeqCst);
    assert_eq!(access.revalidate(), Err(WbsAutomationError::Expired));
    assert_eq!(
        sessions.authorize(view.authorization()).err(),
        Some(WbsAutomationError::Expired)
    );
    let fresh = sessions.offer_document_consent().await?;
    let new_view = sessions.open_session(accept_offer(&fresh)).await?;
    assert_ne!(new_view.authorization(), view.authorization());
    assert_eq!(
        sessions.authorize(new_view.authorization())?.revalidate(),
        Ok(())
    );
    Ok(())
}

#[tokio::test]
async fn document_sessions_recheck_configuration_and_audit_after_waiting() -> TestResult {
    let (fixture, sessions) = document_sessions_fixture(false).await?;
    assert_eq!(
        sessions.offer_document_consent().await,
        Err(WbsAutomationError::AuditUnavailable)
    );
    assert!(sessions.runtime.state()?.offers.is_empty());
    let mut audit = fixture.controller.audit_logger.write().await;
    *audit = session_audit();
    let mut offer_future = Box::pin(sessions.offer_document_consent());
    assert_session_pending(offer_future.as_mut());
    let original = (*fixture.config.snapshot()).clone();
    let mut disabled = original.clone();
    disabled.automation.enabled = false;
    fixture.config.update(disabled.clone())?;
    drop(audit);
    assert_eq!(
        offer_future.await,
        Err(WbsAutomationError::AutomationDisabled)
    );
    assert!(sessions.runtime.state()?.offers.is_empty());
    fixture.config.update(original.clone())?;
    let offer = sessions.offer_document_consent().await?;
    let mut audit = fixture.controller.audit_logger.write().await;
    let mut open_future = Box::pin(sessions.open_session(accept_offer(&offer)));
    assert_session_pending(open_future.as_mut());
    *audit = crate::audit::AuditLogger::new(16, 16);
    drop(audit);
    assert_eq!(open_future.await, Err(WbsAutomationError::AuditUnavailable));
    assert_eq!(sessions.runtime.state()?.offers.len(), 1);
    assert!(sessions.runtime.state()?.sessions.is_empty());
    let mut audit = fixture.controller.audit_logger.write().await;
    *audit = session_audit();
    let mut open_future = Box::pin(sessions.open_session(accept_offer(&offer)));
    assert_session_pending(open_future.as_mut());
    fixture.config.update(disabled)?;
    drop(audit);
    assert_eq!(
        open_future.await,
        Err(WbsAutomationError::AutomationDisabled)
    );
    fixture.config.update(original)?;
    assert_eq!(
        sessions.open_session(accept_offer(&offer)).await,
        Err(WbsAutomationError::ConfigurationChanged)
    );
    let fresh = sessions.offer_document_consent().await?;
    let view = sessions.open_session(accept_offer(&fresh)).await?;
    assert_eq!(
        sessions.authorize(view.authorization())?.revalidate(),
        Ok(())
    );
    Ok(())
}

#[tokio::test]
async fn document_sessions_refuse_a_stalled_audit_lock_without_consuming_the_offer() -> TestResult {
    use std::time::{Duration, Instant};
    let (fixture, sessions) = document_sessions_fixture(true).await?;
    let offer = sessions.offer_document_consent().await?;
    let audit = fixture.controller.audit_logger.write().await;
    let start = Instant::now();
    let result = tokio::time::timeout(
        Duration::from_secs(20),
        sessions.open_session(accept_offer(&offer)),
    )
    .await?;
    assert_eq!(result, Err(WbsAutomationError::AuditUnavailable));
    assert!(start.elapsed() >= Duration::from_secs(7));
    assert_eq!(sessions.runtime.state()?.offers.len(), 1);
    assert!(sessions.runtime.state()?.sessions.is_empty());
    drop(audit);
    let view = sessions.open_session(accept_offer(&offer)).await?;
    assert_eq!(
        sessions.authorize(view.authorization())?.revalidate(),
        Ok(())
    );
    Ok(())
}

async fn assert_nonce_closed(
    controller: &crate::controller::AutomationController,
    pending: &maekon_core::models::automation::PendingConfirmation,
) -> TestResult {
    use maekon_core::ports::automation::AutomationPort;
    assert!(controller.list_pending_confirmations().await?.is_empty());
    let missing = controller
        .submit_confirmation(&pending.command_id, &pending.nonce, true)
        .await
        .expect_err("Completed nonce must be absent");
    assert!(matches!(missing, maekon_core::error::CoreError::NotFound {
        code: maekon_core::error_codes::NotFoundCode::ResourceMissing,
        resource_type,
        id,
    } if resource_type == "PendingConfirmation" && id == pending.command_id));
    Ok(())
}
