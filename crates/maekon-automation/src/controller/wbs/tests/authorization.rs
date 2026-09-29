use super::*;

#[test]
fn signer_key_boundaries_and_debug_never_expose_key_material() -> TestResult {
    for length in [32, 128] {
        let signer = WbsProofSigner::new(Zeroizing::new(vec![7; length]))?;
        assert_eq!(format!("{signer:?}"), "WbsProofSigner([REDACTED])");
    }
    for length in [0, 31, 129] {
        assert_eq!(
            WbsProofSigner::new(Zeroizing::new(vec![7; length]))
                .expect_err("unsupported key length"),
            WbsAutomationError::InvalidInput
        );
    }
    Ok(())
}

#[test]
fn document_consent_session_and_native_payload_match_independent_vectors() -> TestResult {
    let signer = signer(7)?;
    let input = fixture(false, "")?;
    let scope = proof_scope(&input, "")?;
    let consent = proof_consent("")?;
    let acceptance = proof_acceptance(&signer, &scope)?;
    // Independent Python stdlib HMAC vectors use explicitly listed protocol
    // fields and also reproduce the pre-existing candidate vector as a control.
    assert_eq!(
        acceptance.nonce().as_str(),
        "874f2b5628d8ececd5b2128fe5b344189aa3c960d8c58938609dab69a732b2de"
    );
    let offer_expiry = proof_time("2026-09-01T12:01:00Z")?;
    assert_eq!(
        signer.verify_document_consent(&scope, &consent, &acceptance, &offer_expiry),
        Ok(())
    );
    let expiry = proof_time("2026-09-01T12:05:00Z")?;
    let auth = signer.session_authorization(
        &scope,
        &consent,
        &acceptance,
        input.auth.session_id().clone(),
        &expiry,
    )?;
    assert_eq!(
        auth.capability().as_str(),
        "101a2aa077268dec91d0a3b03765648f6500046f3f29f29948ac6bc2d309b49d"
    );
    assert_eq!(
        signer.verify_session_authorization(&scope, &consent, &acceptance, &expiry, &auth),
        Ok(())
    );
    let request = WbsCellApplyRequest::new(
        "request-1".into(),
        input.target.clone(),
        input.candidate.display_name().into(),
    )?;
    let payload = signer.native_payload(
        &input.auth,
        &scope,
        &request,
        &input.board,
        &input.candidate,
    )?;
    assert_eq!(
        payload.as_str(),
        "e46d278dfb68d8b49ca0daec970c9172298af6bdea3ca51fecc345031b515925"
    );
    // Fixed past timestamps still authenticate. Only the controller can enforce
    // current consent, expiry, atomic offer consumption and ticket single use.
    Ok(())
}

#[test]
fn document_proofs_reject_each_consent_identity_change() -> TestResult {
    let signer = signer(7)?;
    let offer_expiry = proof_time("2026-09-01T12:01:00Z")?;
    let expiry = proof_time("2026-09-01T12:05:00Z")?;
    for local in [false, true] {
        let input = fixture(local, "")?;
        let scope = proof_scope(&input, "")?;
        let acceptance = proof_acceptance(&signer, &scope)?;
        let auth = signer.session_authorization(
            &scope,
            &proof_consent("")?,
            &acceptance,
            input.auth.session_id().clone(),
            &expiry,
        )?;
        for field in [
            "consent_id",
            "policy",
            "granted",
            "consent_expiry",
            "no_expiry",
            "process_monitoring",
            "revoked",
            "deletion",
            "erasure",
            "empty_erasure",
        ] {
            let consent = proof_consent(field)?;
            assert_eq!(
                signer.verify_document_consent(&scope, &consent, &acceptance, &offer_expiry),
                Err(WbsAutomationError::InvalidProof),
                "{field}"
            );
            assert_eq!(
                signer.verify_session_authorization(&scope, &consent, &acceptance, &expiry, &auth),
                Err(WbsAutomationError::InvalidProof),
                "{field}"
            );
            let fresh = signer.document_consent_nonce(
                &scope,
                &consent,
                acceptance.offer_id(),
                &offer_expiry,
            )?;
            assert_ne!(fresh, *acceptance.nonce());
        }
    }
    Ok(())
}

#[test]
fn document_proofs_bind_entire_scope_and_local_input_without_a_server() -> TestResult {
    let signer = signer(7)?;
    let consent = proof_consent("")?;
    let offer_expiry = proof_time("2026-09-01T12:01:00Z")?;
    let expiry = proof_time("2026-09-01T12:05:00Z")?;
    for local in [false, true] {
        let original = fixture(local, "")?;
        let scope = proof_scope(&original, "")?;
        let acceptance = proof_acceptance(&signer, &scope)?;
        let auth = signer.session_authorization(
            &scope,
            &consent,
            &acceptance,
            original.auth.session_id().clone(),
            &expiry,
        )?;
        let mut fields = vec!["scope", "registration", "label", "extra_context", "item"];
        fields.extend(if local {
            vec![
                "document",
                "input_revision",
                "input_hash",
                "roster_revision",
                "roster_hash",
            ]
        } else {
            vec![
                "origin",
                "organization",
                "wbs_version",
                "expected_version_absent",
            ]
        });
        for field in fields {
            let changed = fixture(local, field)?;
            let changed_scope = proof_scope(&changed, field)?;
            assert_eq!(
                signer.verify_document_consent(
                    &changed_scope,
                    &consent,
                    &acceptance,
                    &offer_expiry
                ),
                Err(WbsAutomationError::InvalidProof),
                "{field}"
            );
            assert_eq!(
                signer.verify_session_authorization(
                    &changed_scope,
                    &consent,
                    &acceptance,
                    &expiry,
                    &auth
                ),
                Err(WbsAutomationError::InvalidProof),
                "{field}"
            );
            assert_ne!(
                signer.document_consent_nonce(
                    &changed_scope,
                    &consent,
                    acceptance.offer_id(),
                    &offer_expiry
                )?,
                *acceptance.nonce(),
                "{field}"
            );
        }
    }
    Ok(())
}

#[test]
fn document_proofs_reject_acceptance_expiry_session_key_and_domain_substitution() -> TestResult {
    let signer = signer(7)?;
    let foreign = super::super::WbsProofSigner::new(Zeroizing::new(vec![8; 32]))?;
    let input = fixture(true, "")?;
    let scope = proof_scope(&input, "")?;
    let consent = proof_consent("")?;
    let acceptance = proof_acceptance(&signer, &scope)?;
    let offer_expiry = proof_time("2026-09-01T12:01:00Z")?;
    let expiry = proof_time("2026-09-01T12:05:00Z")?;
    let auth = signer.session_authorization(
        &scope,
        &consent,
        &acceptance,
        input.auth.session_id().clone(),
        &expiry,
    )?;
    for field in ["offer", "nonce", "scope", "notice"] {
        let changed = WbsConsentAcceptance::new(
            if field == "offer" {
                WbsIdentifier::new("offer-2".into())?
            } else {
                acceptance.offer_id().clone()
            },
            if field == "nonce" {
                WbsProof::new("f".repeat(64))?
            } else {
                acceptance.nonce().clone()
            },
            if field == "scope" {
                WbsIdentifier::new("scope-2".into())?
            } else {
                acceptance.scope_id().clone()
            },
            if field == "notice" {
                WbsIdentifier::new("notice-2".into())?
            } else {
                acceptance.notice_version().clone()
            },
        );
        assert_eq!(
            signer.verify_document_consent(&scope, &consent, &changed, &offer_expiry),
            Err(WbsAutomationError::InvalidProof),
            "{field}"
        );
        assert_eq!(
            signer.verify_session_authorization(&scope, &consent, &changed, &expiry, &auth),
            Err(WbsAutomationError::InvalidProof),
            "{field}"
        );
        if matches!(field, "scope" | "notice") {
            assert_eq!(
                signer.session_authorization(
                    &scope,
                    &consent,
                    &changed,
                    auth.session_id().clone(),
                    &expiry
                ),
                Err(WbsAutomationError::InvalidProof)
            );
        }
    }
    assert_eq!(
        signer.verify_document_consent(
            &scope,
            &consent,
            &acceptance,
            &(offer_expiry + chrono::Duration::seconds(1))
        ),
        Err(WbsAutomationError::InvalidProof)
    );
    assert_eq!(
        signer.verify_session_authorization(
            &scope,
            &consent,
            &acceptance,
            &(expiry + chrono::Duration::seconds(1)),
            &auth
        ),
        Err(WbsAutomationError::InvalidProof)
    );
    assert_eq!(
        foreign.verify_document_consent(&scope, &consent, &acceptance, &offer_expiry),
        Err(WbsAutomationError::InvalidProof)
    );
    assert_eq!(
        foreign.verify_session_authorization(&scope, &consent, &acceptance, &expiry, &auth),
        Err(WbsAutomationError::InvalidProof)
    );
    let other_session = WbsSessionAuthorization::new(
        WbsIdentifier::new("session-2".into())?,
        auth.capability().clone(),
    );
    assert_eq!(
        signer.verify_session_authorization(&scope, &consent, &acceptance, &expiry, &other_session),
        Err(WbsAutomationError::InvalidProof)
    );
    let crossed =
        WbsSessionAuthorization::new(auth.session_id().clone(), acceptance.nonce().clone());
    assert_eq!(
        signer.verify_session_authorization(&scope, &consent, &acceptance, &expiry, &crossed),
        Err(WbsAutomationError::InvalidProof)
    );
    let crossed = WbsConsentAcceptance::new(
        acceptance.offer_id().clone(),
        auth.capability().clone(),
        acceptance.scope_id().clone(),
        acceptance.notice_version().clone(),
    );
    assert_eq!(
        signer.verify_document_consent(&scope, &consent, &crossed, &offer_expiry),
        Err(WbsAutomationError::InvalidProof)
    );
    Ok(())
}

#[test]
fn consent_observation_refuses_each_live_authority_and_record_denial() -> TestResult {
    use super::super::validated_consent_record;
    use maekon_core::consent::ConsentStatus;
    use std::sync::atomic::Ordering;
    for field in [
        "not_granted",
        "expired_status",
        "update_required",
        "effective",
        "pending",
        "erasure",
        "deletion_flag",
        "erasing",
        "missing_record",
        "record_permission",
        "revoked",
        "record_deletion",
        "record_erasure",
        "expired_record",
        "empty_id",
        "long_id",
        "invalid_id",
        "empty_version",
        "long_version",
    ] {
        let mut authority = ConsentProbe::valid()?;
        let valid = validated_consent_record(&authority)?;
        assert_eq!(
            serde_json::to_value(valid)?,
            serde_json::to_value(authority.record.clone().ok_or("record")?)?
        );
        match field {
            "not_granted" => authority.status = ConsentStatus::NotGranted,
            "expired_status" => authority.status = ConsentStatus::Expired,
            "update_required" => authority.status = ConsentStatus::UpdateRequired,
            "effective" => authority.effective = false,
            "pending" => authority.pending = true,
            "erasure" => authority.erasure = Some("pending-erasure".into()),
            "deletion_flag" => authority.deletion.store(true, Ordering::Release),
            "erasing" => authority.erasing.store(true, Ordering::Release),
            "missing_record" => authority.record = None,
            _ => {
                let record = authority.record.as_mut().ok_or("record")?;
                match field {
                    "record_permission" => record.permissions.process_monitoring = false,
                    "revoked" => record.revoked_at = Some(Utc::now()),
                    "record_deletion" => record.data_deletion_requested = true,
                    "record_erasure" => record.erasure_nonce = Some("record-erasure".into()),
                    "expired_record" => record.expires_at = Some(Utc::now()),
                    "empty_id" => record.consent_id.clear(),
                    "long_id" => record.consent_id = "x".repeat(129),
                    "invalid_id" => record.consent_id = "not an identifier".into(),
                    "empty_version" => record.version.clear(),
                    "long_version" => record.version = "x".repeat(129),
                    _ => unreachable!("Unknown denial case"),
                }
            }
        }
        assert!(
            matches!(
                validated_consent_record(&authority),
                Err(WbsAutomationError::ConsentRequired)
            ),
            "{field}"
        );
    }
    let mut authority = ConsentProbe::valid()?;
    authority.record.as_mut().ok_or("record")?.expires_at = None;
    let valid = validated_consent_record(&authority)?;
    assert_eq!(
        serde_json::to_value(valid)?,
        serde_json::to_value(authority.record.clone().ok_or("record")?)?
    );
    Ok(())
}

#[test]
fn regranted_consent_cannot_authenticate_the_previous_document_proof() -> TestResult {
    use super::super::validated_consent_record;
    let signer = signer(7)?;
    let input = fixture(true, "")?;
    let scope = proof_scope(&input, "")?;
    let mut authority = ConsentProbe::valid()?;
    let old_record = validated_consent_record(&authority)?;
    let offer = WbsIdentifier::new("offer-1".into())?;
    let expiry = Utc::now() + chrono::Duration::seconds(60);
    let old_nonce = signer.document_consent_nonce(&scope, &old_record, &offer, &expiry)?;
    let acceptance = WbsConsentAcceptance::new(
        offer,
        old_nonce,
        scope.scope_id().clone(),
        WbsIdentifier::new(WBS_CONSENT_NOTICE.into())?,
    );
    assert_eq!(
        signer.verify_document_consent(&scope, &old_record, &acceptance, &expiry),
        Ok(())
    );
    authority.effective = false;
    assert!(matches!(
        validated_consent_record(&authority),
        Err(WbsAutomationError::ConsentRequired)
    ));
    authority.effective = true;
    authority.record.as_mut().ok_or("record")?.consent_id = "regranted-consent".into();
    let current = validated_consent_record(&authority)?;
    assert_eq!(
        signer.verify_document_consent(&scope, &current, &acceptance, &expiry),
        Err(WbsAutomationError::InvalidProof)
    );
    Ok(())
}

#[test]
fn live_authority_checks_controller_and_current_configuration_independently() -> TestResult {
    use maekon_core::config::ConfirmationRequirement;
    use std::{sync::Arc, time::Duration};
    for (changed, error) in [
        ("disabled", WbsAutomationError::AutomationDisabled),
        ("sandbox", WbsAutomationError::SandboxUnsupported),
        ("block", WbsAutomationError::PolicyBlocked),
    ] {
        let f = authority_fixture(Arc::new(ConsentProbe::valid()?), changed, true)?;
        assert_eq!(
            f.authority.capture(Duration::from_secs(60)).err(),
            Some(error)
        );
    }
    for (changed, error) in [
        ("disabled", WbsAutomationError::AutomationDisabled),
        ("sandbox", WbsAutomationError::SandboxUnsupported),
        ("block", WbsAutomationError::PolicyBlocked),
    ] {
        let f = authority_fixture(Arc::new(ConsentProbe::valid()?), "", true)?;
        let pin = f.authority.capture(Duration::from_secs(60))?;
        assert_eq!(pin.revalidate(), Ok(()));
        let original = (*f.config.snapshot()).clone();
        let mut current = original.clone();
        match changed {
            "disabled" => current.automation.enabled = false,
            "sandbox" => current.automation.sandbox.enabled = true,
            "block" => current.automation.confirmation_policy = ConfirmationRequirement::Block,
            _ => unreachable!(),
        }
        f.config.update(current)?;
        assert_eq!(pin.revalidate(), Err(error));
        assert_eq!(
            f.authority.capture(Duration::from_secs(60)).err(),
            Some(error)
        );
        f.config.update(original)?;
        assert_eq!(
            pin.revalidate(),
            Err(WbsAutomationError::ConfigurationChanged)
        );
        assert_eq!(
            f.authority.capture(Duration::from_secs(60))?.revalidate(),
            Ok(())
        );
    }
    Ok(())
}

#[test]
fn live_authority_bounds_both_clocks_and_consent_expiry() -> TestResult {
    use std::{
        sync::Arc,
        time::{Duration, Instant},
    };
    let mut consent = ConsentProbe::valid()?;
    let expiry = Utc::now() + chrono::Duration::seconds(30);
    consent.record.as_mut().ok_or("record")?.expires_at = Some(expiry);
    let f = authority_fixture(Arc::new(consent), "", true)?;
    for invalid in [Duration::ZERO, Duration::from_secs(301), Duration::MAX] {
        assert_eq!(
            f.authority.capture(invalid).err(),
            Some(WbsAutomationError::InvalidInput)
        );
    }
    let pin = f.authority.capture(Duration::from_secs(300))?;
    assert_eq!(pin.expires_at(), expiry);
    assert_eq!(pin.revalidate(), Ok(()));
    let remaining = pin.until.saturating_duration_since(Instant::now());
    assert!(remaining > Duration::from_secs(20) && remaining <= Duration::from_secs(30));
    let mut monotonic = pin.clone();
    monotonic.until = Instant::now();
    assert!(monotonic.expires_at() > Utc::now());
    assert_eq!(monotonic.revalidate(), Err(WbsAutomationError::Expired));

    // Keep the monotonic deadline alive to isolate the independent wall-clock check.
    let mut wall = f.authority.capture(Duration::from_millis(500))?;
    wall.until = Instant::now() + Duration::from_secs(60);
    std::thread::sleep(Duration::from_millis(550));
    assert!(wall.until > Instant::now());
    assert!(wall.expires_at() <= Utc::now());
    assert_eq!(wall.revalidate(), Err(WbsAutomationError::Expired));

    let mut consent = ConsentProbe::valid()?;
    consent.record.as_mut().ok_or("record")?.expires_at = None;
    let f = authority_fixture(Arc::new(consent), "", true)?;
    let before = Utc::now();
    let pin = f.authority.capture(Duration::from_secs(300))?;
    assert!(pin.expires_at() >= before + chrono::Duration::seconds(300));
    assert!(pin.expires_at() <= Utc::now() + chrono::Duration::seconds(300));
    assert_eq!(pin.revalidate(), Ok(()));
    Ok(())
}

#[test]
fn live_authority_reuses_current_consent_and_binds_each_identity_field() -> TestResult {
    use std::{sync::Arc, time::Duration};
    let consent = Arc::new(ConsentProbe::valid()?);
    let f = authority_fixture(consent.clone(), "", true)?;
    let pin = f.authority.capture(Duration::from_secs(60))?;
    assert_eq!(pin.revalidate(), Ok(()));
    for changed in ["id", "version", "granted", "expiry"] {
        let mut altered = pin.clone();
        match changed {
            "id" => altered.consent.consent_id = "other-record".into(),
            "version" => altered.consent.version = "other-policy".into(),
            "granted" => altered.consent.granted_at += chrono::Duration::seconds(1),
            "expiry" => altered.consent.expires_at = None,
            _ => unreachable!(),
        }
        assert_eq!(
            altered.revalidate(),
            Err(WbsAutomationError::ConsentChanged),
            "{changed}"
        );
    }
    for flag in [&consent.deletion, &consent.erasing] {
        flag.store(true, std::sync::atomic::Ordering::Release);
        assert_eq!(pin.revalidate(), Err(WbsAutomationError::ConsentRequired));
        assert_eq!(
            f.authority.capture(Duration::from_secs(60)).err(),
            Some(WbsAutomationError::ConsentRequired)
        );
        flag.store(false, std::sync::atomic::Ordering::Release);
    }
    assert_eq!(pin.revalidate(), Ok(()));
    Ok(())
}

#[test]
fn live_authority_regrant_never_restores_a_retained_pin() -> TestResult {
    use maekon_core::consent::{ConsentManager, ConsentPermissions};
    use std::{sync::Arc, time::Duration};
    let directory = tempfile::tempdir()?;
    let consent = Arc::new(ConsentManager::new(directory.path().join("consent.json")));
    let permissions = ConsentPermissions {
        process_monitoring: true,
        ..Default::default()
    };
    consent.grant_consent(permissions.clone(), 7)?;
    let f = authority_fixture(consent.clone(), "", true)?;
    let pin = f.authority.capture(Duration::from_secs(60))?;
    assert_eq!(pin.revalidate(), Ok(()));
    consent.revoke_consent()?;
    assert_eq!(pin.revalidate(), Err(WbsAutomationError::ConsentRequired));
    // Complete only this fixture's erasure, then simulate explicit fresh consent.
    consent.clear_pending_deletion();
    consent.grant_consent(permissions, 7)?;
    assert_eq!(pin.revalidate(), Err(WbsAutomationError::ConsentChanged));
    let fresh = f.authority.capture(Duration::from_secs(60))?;
    assert_ne!(fresh.consent.consent_id, pin.consent.consent_id);
    assert_eq!(fresh.revalidate(), Ok(()));
    Ok(())
}

#[test]
fn live_authority_server_origin_is_scoped_while_local_work_is_independent() -> TestResult {
    use std::{sync::Arc, time::Duration};
    for local in [false, true] {
        for changed in ["origin", "tls"] {
            let f = authority_fixture(Arc::new(ConsentProbe::valid()?), "", local)?;
            let pin = f.authority.capture(Duration::from_secs(60))?;
            assert_eq!(pin.revalidate(), Ok(()));
            let mut current = (*f.config.snapshot()).clone();
            let original = super::super::wbs_server_origin_digest(&current)?;
            match changed {
                "origin" => current.server.base_url = "https://changed.invalid".into(),
                "tls" => current.tls.enabled = !current.tls.enabled,
                _ => unreachable!(),
            }
            assert_ne!(super::super::wbs_server_origin_digest(&current)?, original);
            f.config.update(current)?;
            assert_eq!(
                pin.revalidate(),
                Err(WbsAutomationError::ConfigurationChanged)
            );
            let fresh = f.authority.capture(Duration::from_secs(60));
            if local {
                assert_eq!(fresh?.revalidate(), Ok(()));
            } else {
                assert_eq!(fresh.err(), Some(WbsAutomationError::ConfigurationChanged));
            }
        }
    }
    Ok(())
}

#[test]
fn live_authority_rechecks_configuration_changed_during_consent_observation() -> TestResult {
    use maekon_core::config_manager::ConfigManager;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    };
    use std::time::Duration;
    let slot = Arc::new(Mutex::new(None::<ConfigManager>));
    let reads = Arc::new(AtomicUsize::new(0));
    let mut consent = ConsentProbe::valid()?;
    let captured_slot = slot.clone();
    let captured_reads = reads.clone();
    consent.on_read = Some(Arc::new(move || {
        captured_reads.fetch_add(1, Ordering::SeqCst);
        if let Some(config) = captured_slot.lock().expect("fixture slot").take() {
            // Identical values, new configuration identity, during the first read.
            config
                .update((*config.snapshot()).clone())
                .expect("fixture update");
        }
    }));
    let f = authority_fixture(Arc::new(consent), "", true)?;
    *slot.lock().expect("fixture slot") = Some(f.config.clone());
    assert_eq!(
        f.authority.capture(Duration::from_secs(60)).err(),
        Some(WbsAutomationError::ConfigurationChanged)
    );
    assert_eq!(reads.load(Ordering::SeqCst), 2);
    let pin = f.authority.capture(Duration::from_secs(60))?;
    assert_eq!(reads.load(Ordering::SeqCst), 4);
    assert_eq!(pin.revalidate(), Ok(()));
    Ok(())
}

pub(super) mod modal_confirmation {
    use super::*;
    use crate::controller::wbs::{WbsDocumentSessionAccess, WbsDocumentSessions, WbsLiveAuthority};
    use crate::{controller::AutomationController, policy::PolicyClient, sandbox::NoOpSandbox};
    use maekon_core::config::{ConfirmationRequirement, SandboxConfig};
    use maekon_core::models::automation::PendingConfirmation;
    use maekon_core::ports::automation::AutomationPort;
    use maekon_core::ports::consent_manager::ConsentManagerPort;
    use std::sync::atomic::Ordering;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    pub(in crate::controller::wbs::tests) async fn modal_fixture(
        config_confirm: bool,
        controller_confirm: bool,
        callback: Option<Arc<dyn Fn(PendingConfirmation) + Send + Sync>>,
    ) -> TestResult<(
        AuthorityFixture,
        WbsDocumentSessions,
        WbsDocumentSessionAccess,
        Arc<ConsentProbe>,
    )> {
        let consent = Arc::new(ConsentProbe::valid()?);
        let mut f = authority_fixture(consent.clone(), "", true)?;
        let mut config = (*f.config.snapshot()).clone();
        config.automation.confirmation_policy = if config_confirm {
            ConfirmationRequirement::Confirm
        } else {
            ConfirmationRequirement::Auto
        };
        f.config.update(config)?;
        *f.controller.audit_logger.write().await = session_audit();
        let mut controller = AutomationController::new(
            Arc::new(PolicyClient::new()),
            f.controller.audit_logger.clone(),
            Arc::new(NoOpSandbox),
            SandboxConfig {
                enabled: false,
                ..Default::default()
            },
        );
        if let Some(callback) = callback {
            controller = controller.with_confirmation_callback(callback);
        }
        controller.set_enabled(true);
        controller.set_confirmation_policy(if controller_confirm {
            ConfirmationRequirement::Confirm
        } else {
            ConfirmationRequirement::Auto
        });
        f.controller = Arc::new(controller);
        f.authority = WbsLiveAuthority::new(
            f.controller.clone(),
            f.config.clone(),
            consent.clone(),
            f.scope.clone(),
        );
        let documents = WbsDocumentSessions::new(
            f.controller.clone(),
            f.config.clone(),
            consent.clone(),
            f.scope.clone(),
            Zeroizing::new(vec![9; 32]),
        )?;
        let offer = documents.offer_document_consent().await?;
        let session = documents.open_session(accept_offer(&offer)).await?;
        let access = documents.authorize(session.authorization())?;
        Ok((f, documents, access, consent))
    }

    fn start(
        access: WbsDocumentSessionAccess,
    ) -> tokio::task::JoinHandle<Result<(), WbsAutomationError>> {
        tokio::spawn(async move {
            access
                .confirm_cell_apply(&WbsIdentifier::new("modal-operation".into())?)
                .await
        })
    }

    #[tokio::test]
    async fn modal_gate_honors_both_policies_and_nonce_bound_approval_or_denial() -> TestResult {
        let (f, _, access, _) = modal_fixture(false, false, None).await?;
        access
            .confirm_cell_apply(&WbsIdentifier::new("auto-operation".into())?)
            .await?;
        assert!(f.controller.list_pending_confirmations().await?.is_empty());
        for (config_confirm, controller_confirm) in [(true, false), (false, true)] {
            for approved in [false, true] {
                let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
                let (f, _, access, _) = modal_fixture(
                    config_confirm,
                    controller_confirm,
                    Some(Arc::new(move |value| {
                        sender
                            .send(value)
                            .expect("Synthetic modal receiver remains alive");
                    })),
                )
                .await?;
                let task = start(access);
                let pending = tokio::time::timeout(Duration::from_secs(2), receiver.recv())
                    .await?
                    .ok_or("modal")?;
                assert_eq!(pending.command_id, "modal-operation");
                // Derived from the real nonce so it always differs from it,
                // rather than a literal that reads as a hard-coded nonce (#12745).
                let forged = format!("forged-{}", pending.nonce);
                let denied = f
                    .controller
                    .submit_confirmation(&pending.command_id, &forged, true)
                    .await
                    .expect_err("Forged nonce must be rejected");
                assert!(
                    matches!(denied, maekon_core::error::CoreError::PermissionDenied {
                    code: maekon_core::error_codes::PermissionCode::PermissionDenied,
                    message,
                } if message == format!("confirm automation command '{}': nonce mismatch", pending.command_id))
                );
                let retained = f.controller.list_pending_confirmations().await?;
                assert_eq!(retained.len(), 1);
                assert_eq!(retained[0].nonce, pending.nonce);
                f.controller
                    .submit_confirmation(&pending.command_id, &pending.nonce, approved)
                    .await?;
                let result = tokio::time::timeout(Duration::from_secs(2), task).await??;
                assert_eq!(
                    result,
                    if approved {
                        Ok(())
                    } else {
                        Err(WbsAutomationError::ConfirmationDenied)
                    }
                );
                assert_nonce_closed(f.controller.as_ref(), &pending).await?;
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn modal_gate_rechecks_cancel_configuration_erasure_and_expiry_while_waiting(
    ) -> TestResult {
        for change in ["cancel", "configuration", "erasure", "expiry"] {
            let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
            let (f, documents, mut access, consent) = modal_fixture(
                true,
                true,
                Some(Arc::new(move |value| {
                    sender
                        .send(value)
                        .expect("Synthetic modal receiver remains alive");
                })),
            )
            .await?;
            if change == "expiry" {
                access.pin.until = Instant::now() + Duration::from_millis(250);
            }
            let task = start(access.clone());
            let pending = tokio::time::timeout(Duration::from_secs(2), receiver.recv())
                .await?
                .ok_or("modal")?;
            let expected = match change {
                "cancel" => {
                    documents.cancel(access.view().authorization())?;
                    WbsAutomationError::Expired
                }
                "configuration" => {
                    let mut config = (*f.config.snapshot()).clone();
                    config.automation.confirmation_policy = ConfirmationRequirement::Auto;
                    f.config.update(config)?;
                    WbsAutomationError::ConfigurationChanged
                }
                "erasure" => {
                    consent.erasing().store(true, Ordering::Release);
                    WbsAutomationError::ConsentRequired
                }
                _ => WbsAutomationError::Expired,
            };
            let result = tokio::time::timeout(Duration::from_secs(2), task).await??;
            if change == "expiry" {
                assert!(matches!(
                    result,
                    Err(WbsAutomationError::Expired | WbsAutomationError::ConfirmationDenied)
                ));
            } else {
                assert_eq!(result, Err(expected));
            }
            assert_nonce_closed(f.controller.as_ref(), &pending).await?;
        }
        Ok(())
    }

    #[tokio::test]
    async fn modal_gate_cleans_nonce_after_caller_drop_lock_contention_and_callback_panic(
    ) -> TestResult {
        for action in ["drop", "contended_drop", "panic"] {
            let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
            let (f, _, access, _) = modal_fixture(
                true,
                true,
                Some(Arc::new(move |value| {
                    sender
                        .send(value)
                        .expect("Synthetic modal receiver remains alive");
                    assert_ne!(action, "panic", "Synthetic confirmation callback failure");
                })),
            )
            .await?;
            let task = start(access);
            let pending = tokio::time::timeout(Duration::from_secs(2), receiver.recv())
                .await?
                .ok_or("modal")?;
            if action == "panic" {
                let error = task.await.err().ok_or("callback did not panic")?;
                assert!(error.is_panic());
            } else {
                let held = if action == "contended_drop" {
                    Some(f.controller.pending_confirmations.lock().await)
                } else {
                    None
                };
                task.abort();
                let error = task.await.err().ok_or("caller did not cancel")?;
                assert!(error.is_cancelled());
                if let Some(ref held) = held {
                    assert!(held.contains_key(&pending.command_id));
                }
                drop(held);
            }
            tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    if f.controller.list_pending_confirmations().await?.is_empty() {
                        break Ok::<(), maekon_core::error::CoreError>(());
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await??;
            assert_nonce_closed(f.controller.as_ref(), &pending).await?;
        }
        Ok(())
    }
}
