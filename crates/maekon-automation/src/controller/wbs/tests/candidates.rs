use super::*;

#[test]
fn candidate_proof_matches_independent_framing_vector_and_verifies() -> TestResult {
    let signer = signer(7)?;
    let input = fixture(false, "")?;
    let proof = prove(&signer, &input)?;
    // Fixed independently with Python's stdlib HMAC over the explicitly listed
    // synthetic protocol fields; this value is not computed by this signer test.
    assert_eq!(
        proof.as_str(),
        "415728f5f38cf51f504751d37926645ba813847cac5f012b12c43546acc13ccc"
    );
    assert_eq!(verify(&signer, &input, &proof), Ok(()));
    let local = fixture(true, "")?;
    let local_proof = prove(&signer, &local)?;
    assert_eq!(verify(&signer, &local, &local_proof), Ok(()));
    assert_ne!(local_proof, proof);
    Ok(())
}

#[test]
fn candidate_proof_rejects_changes_to_each_shared_target_and_result_pin() -> TestResult {
    let signer = signer(7)?;
    let fields = [
        "binding",
        "binding_generation",
        "process",
        "process_start",
        "workbook",
        "worksheet",
        "row",
        "column",
        "before",
        "item",
        "query_generation",
        "query_limit",
        "snapshot_id",
        "snapshot_version",
        "snapshot_hash",
        "session",
        "capability",
        "candidate_id",
        "candidate_hash",
        "user_id",
        "display_name",
        "rank",
        "eligible",
        "reason",
        "algorithm",
        "algorithm_version",
        "algorithm_as_of",
        "synthetic",
        "provenance",
    ];
    for local in [false, true] {
        let original = fixture(local, "")?;
        let proof = prove(&signer, &original)?;
        for field in fields {
            let changed = fixture(local, field)?;
            assert_ne!(
                prove(&signer, &changed)?,
                proof,
                "field={field} local={local}"
            );
            assert_eq!(
                verify(&signer, &changed, &proof),
                Err(WbsAutomationError::InvalidProof),
                "field={field} local={local}"
            );
        }
    }
    Ok(())
}

#[test]
fn candidate_proof_binds_local_inputs_and_server_approval_without_conflating_them() -> TestResult {
    let signer = signer(7)?;
    for (local, fields) in [
        (
            true,
            vec![
                "document",
                "input_revision",
                "input_hash",
                "roster_revision",
                "roster_hash",
                "provider_selection",
                "provider_name",
                "model",
                "prompt_revision",
            ],
        ),
        (
            false,
            vec![
                "organization",
                "wbs_version",
                "expected_version_absent",
                "wbs_hash",
                "approval",
            ],
        ),
    ] {
        let proof = prove(&signer, &fixture(local, "")?)?;
        for field in fields {
            let changed = fixture(local, field)?;
            assert_ne!(prove(&signer, &changed)?, proof, "field={field}");
            assert_eq!(
                verify(&signer, &changed, &proof),
                Err(WbsAutomationError::InvalidProof),
                "field={field}"
            );
        }
    }
    Ok(())
}

#[test]
fn candidate_proof_rejects_other_keys_tampering_crossed_contexts_and_missing_source() -> TestResult
{
    let signer = signer(7)?;
    let input = fixture(true, "")?;
    let proof = prove(&signer, &input)?;
    let other_key = WbsProofSigner::new(Zeroizing::new(vec![8; 32]))?;
    assert_eq!(
        verify(&other_key, &input, &proof),
        Err(WbsAutomationError::InvalidProof)
    );
    let mut tampered = proof.as_str().as_bytes().to_vec();
    tampered[0] = if tampered[0] == b'0' { b'1' } else { b'0' };
    assert_eq!(
        verify(
            &signer,
            &input,
            &WbsProof::new(String::from_utf8(tampered)?)?
        ),
        Err(WbsAutomationError::InvalidProof)
    );
    let server = fixture(false, "")?;
    assert_eq!(
        signer.candidate_proof(&input.auth, &input.target, &server.board, &server.candidate),
        Err(WbsAutomationError::InvalidProof)
    );
    // A valid candidate from a different result must not be endorsed against
    // this board, even when its identifier or displayed text is unchanged.
    for field in ["candidate_id", "snapshot_id", "display_name"] {
        let outsider = fixture(true, field)?;
        assert_eq!(
            signer.candidate_proof(
                &input.auth,
                &input.target,
                &input.board,
                &outsider.candidate
            ),
            Err(WbsAutomationError::InvalidProof),
            "field={field}"
        );
        assert_eq!(
            signer.verify_candidate(
                &input.auth,
                &input.target,
                &input.board,
                &outsider.candidate,
                &proof
            ),
            Err(WbsAutomationError::InvalidProof),
            "field={field}"
        );
    }
    let empty = WbsAssignmentCandidates::empty(
        input.board.query().clone(),
        input.board.provenance().clone(),
    );
    assert_eq!(
        signer.candidate_proof(&input.auth, &input.target, &empty, &input.candidate),
        Err(WbsAutomationError::StaleCandidate)
    );
    Ok(())
}

mod runtime_reads {
    use super::*;
    use crate::audit::{AuditEntry, AuditLogger, AuditPersistError, AuditPersistence, AuditStatus};
    use crate::controller::wbs::{WbsAutomationService, WbsDocumentSessions};
    use maekon_core::ports::consent_manager::ConsentManagerPort;
    use maekon_core::ports::wbs_assignment_candidates::WbsAssignmentCandidatesPort;
    use maekon_core::ports::wbs_cell_target::WbsCellTargetPort;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};
    use tokio::sync::Semaphore;

    struct CellProbe {
        target: Mutex<WbsCellTargetSnapshot>,
        captures: AtomicUsize,
        validations: AtomicUsize,
        writes: AtomicUsize,
    }
    #[async_trait::async_trait]
    impl WbsCellTargetPort for CellProbe {
        async fn capture_selection(&self) -> Result<WbsCellTargetSnapshot, WbsCellTargetError> {
            self.captures.fetch_add(1, Ordering::SeqCst);
            Ok(self
                .target
                .lock()
                .map_err(|_| WbsCellTargetError::Unavailable)?
                .clone())
        }
        async fn validate_bound_cell(
            &self,
            target: &WbsCellTargetSnapshot,
        ) -> Result<(), WbsCellTargetError> {
            self.validations.fetch_add(1, Ordering::SeqCst);
            let current = self
                .target
                .lock()
                .map_err(|_| WbsCellTargetError::Unavailable)?;
            target.ensure_unchanged(&current)
        }
        async fn read_bound_cell(
            &self,
            _: &WbsCellTargetSnapshot,
        ) -> Result<WbsBoundCellReadback, WbsCellTargetError> {
            panic!("Recommendation must not invoke execution readback")
        }
        async fn apply_bound_cell(
            &self,
            _: &WbsCellApplyRequest,
            _: Instant,
        ) -> Result<WbsCellWriteOutcome, WbsCellTargetError> {
            self.writes.fetch_add(1, Ordering::SeqCst);
            Err(WbsCellTargetError::Unsupported)
        }
    }

    struct ProviderProbe {
        template: WbsAssignmentCandidates,
        calls: AtomicUsize,
        blocked: AtomicBool,
        wrong_query: AtomicBool,
        empty: AtomicBool,
        entered: Semaphore,
        release: Semaphore,
    }
    impl ProviderProbe {
        fn board(
            &self,
            query: &WbsCandidateQuery,
        ) -> Result<WbsAssignmentCandidates, WbsCandidateError> {
            let query = if self.wrong_query.load(Ordering::SeqCst) {
                WbsCandidateQuery::new(
                    query.context().clone(),
                    query.generation() + 1,
                    query.limit(),
                )?
            } else {
                query.clone()
            };
            if self.empty.load(Ordering::SeqCst) {
                return Ok(WbsAssignmentCandidates::empty(
                    query,
                    self.template.provenance().clone(),
                ));
            }
            let provenance = self.template.provenance().clone();
            let candidates = self.template.candidates().to_vec();
            match self
                .template
                .source()
                .ok_or(WbsCandidateError::InvalidResponse)?
            {
                WbsCandidateSource::ServerBoard(value) => {
                    WbsAssignmentCandidates::ready(query, value.clone(), provenance, candidates)
                }
                WbsCandidateSource::LocalDocumentRoster(value) => {
                    WbsAssignmentCandidates::ready_local(
                        query,
                        value.clone(),
                        provenance,
                        candidates,
                    )
                }
            }
        }
        async fn wait_entered(&self) -> TestResult {
            tokio::time::timeout(Duration::from_secs(2), self.entered.acquire())
                .await??
                .forget();
            Ok(())
        }
    }
    #[async_trait::async_trait]
    impl WbsAssignmentCandidatesPort for ProviderProbe {
        async fn recommend_assignees(
            &self,
            query: &WbsCandidateQuery,
        ) -> Result<WbsAssignmentCandidates, WbsCandidateError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.entered.add_permits(1);
            if self.blocked.load(Ordering::SeqCst) {
                self.release
                    .acquire()
                    .await
                    .map_err(|_| WbsCandidateError::Unavailable)?
                    .forget();
            }
            self.board(query)
        }
    }

    #[derive(Default)]
    struct AuditProbe {
        calls: AtomicUsize,
        reject_at: AtomicUsize,
        accepted: Mutex<Vec<AuditEntry>>,
    }
    impl AuditPersistence for AuditProbe {
        fn persist(&self, _: &AuditEntry) {
            panic!("Full reads must use checked persistence")
        }
        fn persist_checked(&self, entry: &AuditEntry) -> Result<(), AuditPersistError> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            if call == self.reject_at.load(Ordering::SeqCst) {
                return Err(AuditPersistError::ChannelFull);
            }
            self.accepted
                .lock()
                .map_err(|_| AuditPersistError::ChannelClosed)?
                .push(entry.clone());
            Ok(())
        }
    }

    struct RuntimeFixture {
        authority: AuthorityFixture,
        consent: Arc<ConsentProbe>,
        documents: WbsDocumentSessions,
        service: WbsAutomationService,
        auth: WbsSessionAuthorization,
        target: Arc<CellProbe>,
        provider: Arc<ProviderProbe>,
        audit: Arc<AuditProbe>,
    }
    async fn runtime_fixture(local: bool) -> TestResult<RuntimeFixture> {
        let original = fixture(local, "")?;
        let consent = Arc::new(ConsentProbe::valid()?);
        let authority = authority_fixture(consent.clone(), "", local)?;
        let audit = Arc::new(AuditProbe::default());
        *authority.controller.audit_logger.write().await =
            AuditLogger::new(128, 16).with_persistence(audit.clone());
        let documents = WbsDocumentSessions::new(
            authority.controller.clone(),
            authority.config.clone(),
            consent.clone(),
            authority.scope.clone(),
            Zeroizing::new(vec![9; 32]),
        )?;
        let observed = WbsCellTargetSnapshot::new(
            original.target.binding().clone(),
            original.target.identity().clone(),
            original.target.before_value().into(),
            Some(WbsCellAnchor::new(-100, 50, 80, 20, 144)?),
        )?;
        let target = Arc::new(CellProbe {
            target: Mutex::new(observed),
            captures: AtomicUsize::new(0),
            validations: AtomicUsize::new(0),
            writes: AtomicUsize::new(0),
        });
        let provider = Arc::new(ProviderProbe {
            template: original.board,
            calls: AtomicUsize::new(0),
            blocked: AtomicBool::new(false),
            wrong_query: AtomicBool::new(false),
            empty: AtomicBool::new(false),
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
        });
        let service =
            WbsAutomationService::new(documents.clone(), target.clone(), provider.clone());
        let offer = service.offer_document_consent().await?;
        let session = service.open_session(accept_offer(&offer)).await?;
        Ok(RuntimeFixture {
            authority,
            consent,
            documents,
            service,
            auth: session.authorization().clone(),
            target,
            provider,
            audit,
        })
    }
    fn start(
        f: &RuntimeFixture,
    ) -> tokio::task::JoinHandle<Result<WbsRecommendationView, WbsAutomationError>> {
        let service = f.service.clone();
        let auth = f.auth.clone();
        tokio::spawn(async move { service.recommend(auth).await })
    }
    fn audit_statuses(f: &RuntimeFixture) -> TestResult<Vec<AuditStatus>> {
        let entries = f.audit.accepted.lock().map_err(|_| "audit poisoned")?;
        assert!(entries
            .iter()
            .all(|row| row.action_type == "wbs_assignee_context_read"));
        Ok(entries.iter().map(|row| row.status.clone()).collect())
    }

    #[tokio::test]
    async fn recommendation_reads_preserve_local_and_server_proofs_generations_and_empty_results(
    ) -> TestResult {
        for local in [false, true] {
            let f = runtime_fixture(local).await?;
            let target = f
                .target
                .target
                .lock()
                .map_err(|_| "target poisoned")?
                .clone();
            let mut proofs = Vec::new();
            for generation in [1, 2] {
                let result = f.service.recommend(f.auth.clone()).await?;
                assert_eq!(result.query_generation().value(), generation);
                assert_eq!(result.candidates().len(), 1);
                assert_eq!(result.anchor(), target.anchor());
                let query =
                    WbsCandidateQuery::new(target.identity().context().clone(), generation, 10)?;
                let board = f.provider.board(&query)?;
                let card = &result.candidates()[0];
                assert_eq!(
                    card.display_name().as_str(),
                    board.candidates()[0].display_name()
                );
                signer(9)?.verify_candidate(
                    &f.auth,
                    &target,
                    &board,
                    &board.candidates()[0],
                    card.candidate_proof(),
                )?;
                proofs.push(card.candidate_proof().clone());
                assert_eq!(
                    serde_json::to_value(&result)?,
                    serde_json::to_value(f.service.read_recommendation(&f.auth)?)?
                );
            }
            assert_ne!(proofs[0], proofs[1]);
            f.provider.empty.store(true, Ordering::SeqCst);
            let empty = f.service.recommend(f.auth.clone()).await?;
            assert!(empty.candidates().is_empty() && empty.source().is_none());
            assert_eq!(empty.query_generation().value(), 3);
            assert_eq!(f.target.captures.load(Ordering::SeqCst), 3);
            assert_eq!(f.target.validations.load(Ordering::SeqCst), 3);
            assert_eq!(f.provider.calls.load(Ordering::SeqCst), 3);
            assert_eq!(f.target.writes.load(Ordering::SeqCst), 0);
            assert_eq!(
                audit_statuses(&f)?,
                vec![
                    AuditStatus::Started,
                    AuditStatus::Completed,
                    AuditStatus::Started,
                    AuditStatus::Completed,
                    AuditStatus::Started,
                    AuditStatus::Completed
                ]
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn recommendation_reads_refuse_capability_scope_and_generation_overflow_before_provider(
    ) -> TestResult {
        let f = runtime_fixture(true).await?;
        let bad = WbsSessionAuthorization::new(
            f.auth.session_id().clone(),
            WbsProof::new("0".repeat(64))?,
        );
        assert_eq!(
            f.service.recommend(bad).await.err(),
            Some(WbsAutomationError::InvalidCapability)
        );
        assert_eq!(f.target.captures.load(Ordering::SeqCst), 0);
        *f.target.target.lock().map_err(|_| "target poisoned")? = fixture(true, "item")?.target;
        assert_eq!(
            f.service.recommend(f.auth.clone()).await.err(),
            Some(WbsAutomationError::StaleSelection)
        );
        assert_eq!(f.provider.calls.load(Ordering::SeqCst), 0);
        f.documents.runtime.state()?.generation = u64::MAX;
        assert_eq!(
            f.service.recommend(f.auth.clone()).await.err(),
            Some(WbsAutomationError::CapacityExceeded)
        );
        assert_eq!(f.target.captures.load(Ordering::SeqCst), 1);
        assert_eq!(f.target.writes.load(Ordering::SeqCst), 0);
        Ok(())
    }

    #[tokio::test]
    async fn recommendation_reads_require_both_audit_acceptances_and_clear_old_results(
    ) -> TestResult {
        for reject_at in [1, 2] {
            let f = runtime_fixture(true).await?;
            f.audit.reject_at.store(reject_at, Ordering::SeqCst);
            assert_eq!(
                f.service.recommend(f.auth.clone()).await.err(),
                Some(WbsAutomationError::AuditUnavailable)
            );
            assert_eq!(f.provider.calls.load(Ordering::SeqCst), reject_at - 1);
            assert_eq!(
                f.service.read_recommendation(&f.auth).err(),
                Some(WbsAutomationError::StaleCandidate)
            );
            f.audit.reject_at.store(0, Ordering::SeqCst);
            f.service.recommend(f.auth.clone()).await?;
            f.provider.wrong_query.store(true, Ordering::SeqCst);
            assert_eq!(
                f.service.recommend(f.auth.clone()).await.err(),
                Some(WbsAutomationError::StaleCandidate)
            );
            assert_eq!(
                f.service.read_recommendation(&f.auth).err(),
                Some(WbsAutomationError::StaleCandidate)
            );
            assert_eq!(f.target.writes.load(Ordering::SeqCst), 0);
        }
        let f = runtime_fixture(true).await?;
        *f.authority.controller.audit_logger.write().await = AuditLogger::new(16, 16);
        assert_eq!(
            f.service.recommend(f.auth.clone()).await.err(),
            Some(WbsAutomationError::AuditUnavailable)
        );
        assert_eq!(f.target.captures.load(Ordering::SeqCst), 0);
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn recommendation_reads_reject_cancel_config_and_erasure_while_provider_waits(
    ) -> TestResult {
        for change in ["cancel", "config", "erasure"] {
            let f = runtime_fixture(true).await?;
            f.provider.blocked.store(true, Ordering::SeqCst);
            let task = start(&f);
            f.provider.wait_entered().await?;
            let error = match change {
                "cancel" => {
                    f.documents.cancel(&f.auth)?;
                    WbsAutomationError::Expired
                }
                "config" => {
                    let mut changed = (*f.authority.config.snapshot()).clone();
                    changed.automation.confirmation_policy =
                        maekon_core::config::ConfirmationRequirement::Confirm;
                    f.authority.config.update(changed)?;
                    WbsAutomationError::ConfigurationChanged
                }
                _ => {
                    f.consent.erasing().store(true, Ordering::Release);
                    WbsAutomationError::ConsentRequired
                }
            };
            assert_eq!(
                tokio::time::timeout(Duration::from_secs(2), task)
                    .await??
                    .err(),
                Some(error)
            );
            assert_eq!(f.provider.calls.load(Ordering::SeqCst), 1);
            assert_eq!(f.target.validations.load(Ordering::SeqCst), 0);
            assert_eq!(
                audit_statuses(&f)?,
                vec![AuditStatus::Started, AuditStatus::Failed]
            );
            assert_eq!(f.target.writes.load(Ordering::SeqCst), 0);
        }
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn recommendation_reads_reject_changed_original_selection_after_provider() -> TestResult {
        let f = runtime_fixture(true).await?;
        f.provider.blocked.store(true, Ordering::SeqCst);
        let task = start(&f);
        f.provider.wait_entered().await?;
        *f.target.target.lock().map_err(|_| "target poisoned")? = fixture(true, "before")?.target;
        f.provider.release.add_permits(1);
        assert_eq!(task.await?.err(), Some(WbsAutomationError::StaleSelection));
        assert_eq!(f.target.validations.load(Ordering::SeqCst), 1);
        assert_eq!(
            f.service.read_recommendation(&f.auth).err(),
            Some(WbsAutomationError::StaleCandidate)
        );
        assert_eq!(
            audit_statuses(&f)?,
            vec![AuditStatus::Started, AuditStatus::Failed]
        );
        assert_eq!(f.target.writes.load(Ordering::SeqCst), 0);
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn recommendation_reads_keep_busy_after_caller_drop_until_owned_worker_finishes(
    ) -> TestResult {
        let f = runtime_fixture(true).await?;
        f.provider.blocked.store(true, Ordering::SeqCst);
        let task = start(&f);
        f.provider.wait_entered().await?;
        task.abort();
        let cancelled = task.await.err().ok_or("caller was not cancelled")?;
        assert!(cancelled.is_cancelled());
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), f.service.recommend(f.auth.clone()))
                .await?
                .err(),
            Some(WbsAutomationError::Busy)
        );
        assert_eq!(f.provider.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            f.service.read_recommendation(&f.auth).err(),
            Some(WbsAutomationError::StaleCandidate)
        );
        f.provider.release.add_permits(1);
        let view = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Ok(view) = f.service.read_recommendation(&f.auth) {
                    break view;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await?;
        assert_eq!(view.candidates().len(), 1);
        f.provider.blocked.store(false, Ordering::SeqCst);
        let next = f.service.recommend(f.auth.clone()).await?;
        assert_eq!(next.query_generation().value(), 2);
        assert_eq!(f.provider.calls.load(Ordering::SeqCst), 2);
        assert_eq!(f.target.writes.load(Ordering::SeqCst), 0);
        Ok(())
    }

    #[tokio::test]
    async fn recommendation_reads_expire_waiting_provider_and_recheck_after_audit_lock(
    ) -> TestResult {
        let f = runtime_fixture(true).await?;
        f.documents
            .runtime
            .state()?
            .sessions
            .get_mut(f.auth.session_id().as_str())
            .ok_or("session")?
            .pin
            .until = Instant::now() + Duration::from_secs(1);
        f.provider.blocked.store(true, Ordering::SeqCst);
        let task = start(&f);
        f.provider.wait_entered().await?;
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(2), task).await??,
            Err(WbsAutomationError::Expired | WbsAutomationError::Unavailable)
        ));
        assert_eq!(f.target.validations.load(Ordering::SeqCst), 0);
        assert_eq!(
            audit_statuses(&f)?,
            vec![AuditStatus::Started, AuditStatus::Failed]
        );
        let f = runtime_fixture(true).await?;
        let guard = f.authority.controller.audit_logger.write().await;
        let task = start(&f);
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let busy = f
                    .documents
                    .runtime
                    .state()?
                    .sessions
                    .get(f.auth.session_id().as_str())
                    .ok_or(WbsAutomationError::InvalidCapability)?
                    .busy;
                if busy {
                    break Ok::<(), WbsAutomationError>(());
                }
                tokio::task::yield_now().await;
            }
        })
        .await??;
        let mut disabled = (*f.authority.config.snapshot()).clone();
        disabled.automation.enabled = false;
        f.authority.config.update(disabled)?;
        drop(guard);
        assert_eq!(
            task.await?.err(),
            Some(WbsAutomationError::AutomationDisabled)
        );
        assert_eq!(f.target.captures.load(Ordering::SeqCst), 0);
        assert_eq!(f.provider.calls.load(Ordering::SeqCst), 0);
        Ok(())
    }

    #[tokio::test]
    async fn recommendation_native_reads_reject_expired_pending_and_late_ready_results(
    ) -> TestResult {
        use crate::controller::wbs::execution::native_read;
        let calls = AtomicUsize::new(0);
        let expired = native_read(Instant::now() - Duration::from_secs(1), async {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(17)
        })
        .await;
        assert_eq!(expired, Err(WbsAutomationError::StaleSelection));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            native_read(Instant::now() + Duration::from_secs(2), async {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(25)
            })
            .await,
            Ok(25)
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            native_read(
                Instant::now() + Duration::from_millis(10),
                std::future::pending::<Result<(), WbsCellTargetError>>(),
            )
            .await,
            Err(WbsAutomationError::StaleSelection)
        );
        let until = Instant::now() + Duration::from_millis(250);
        let late = native_read(until, async {
            // Deliberately model a native call that does not yield to Tokio.
            std::thread::sleep(
                until.saturating_duration_since(Instant::now()) + Duration::from_millis(10),
            );
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(42)
        })
        .await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(late, Err(WbsAutomationError::StaleSelection));
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn recommendation_reads_discard_released_and_superseded_workers() -> TestResult {
        for superseded in [false, true] {
            let f = runtime_fixture(true).await?;
            f.provider.blocked.store(true, Ordering::SeqCst);
            let task = start(&f);
            f.provider.wait_entered().await?;
            {
                let mut state = f.documents.runtime.state()?;
                let session = state
                    .sessions
                    .get_mut(f.auth.session_id().as_str())
                    .ok_or("session")?;
                assert!(session.busy);
                assert_eq!(session.generation, 1);
                if superseded {
                    session.generation = 2;
                } else {
                    session.busy = false;
                }
            }
            f.provider.release.add_permits(1);
            assert_eq!(task.await?.err(), Some(WbsAutomationError::StaleCandidate));
            assert_eq!(
                f.service.read_recommendation(&f.auth).err(),
                Some(WbsAutomationError::StaleCandidate)
            );
            let state = f.documents.runtime.state()?;
            let session = state
                .sessions
                .get(f.auth.session_id().as_str())
                .ok_or("session")?;
            assert_eq!(
                session.busy, superseded,
                "Old work must not clear a newer reservation"
            );
            assert_eq!(f.target.writes.load(Ordering::SeqCst), 0);
        }
        Ok(())
    }
}
