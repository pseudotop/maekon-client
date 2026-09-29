use super::*;

fn execution_ticket_fixture(
    signer: &WbsProofSigner,
    input: &ProofFixture,
) -> TestResult<WbsExecutionTicket> {
    Ok(signer.execution_ticket(
        &input.auth,
        WbsIdentifier::new("operation-1".into())?,
        WbsProof::new("a".repeat(64))?,
        DateTime::parse_from_rfc3339("2026-09-01T12:01:00Z")?.with_timezone(&Utc),
    )?)
}

#[test]
fn execution_ticket_matches_independent_framing_vector() -> TestResult {
    let signer = signer(7)?;
    let input = fixture(true, "")?;
    let ticket = execution_ticket_fixture(&signer, &input)?;
    // Independently calculated with Python stdlib HMAC and explicit wire fields.
    // The timestamp is data here; the controller separately enforces expiry.
    assert_eq!(
        ticket.nonce().as_str(),
        "c69deeba1c80e12dc9ab28ed24dc23af8fba83125c241539fb821af5bc56b334"
    );
    assert_eq!(
        ticket.signature().as_str(),
        "05438436b815f24e2c508bdbf31fda4b01eb431d90685e284719449f88b48f44"
    );
    assert_eq!(ticket.operation_id().as_str(), "operation-1");
    assert_eq!(ticket.session_id(), input.auth.session_id());
    assert_eq!(ticket.payload_digest().as_str(), "a".repeat(64));
    assert_eq!(signer.verify_execution_ticket(&input.auth, &ticket), Ok(()));
    Ok(())
}

#[test]
fn execution_ticket_rejects_each_changed_field_and_foreign_authorization() -> TestResult {
    let signer = signer(7)?;
    let input = fixture(true, "")?;
    let ticket = execution_ticket_fixture(&signer, &input)?;
    for field in [
        "operation",
        "session",
        "nonce",
        "payload",
        "expiry",
        "signature",
    ] {
        let changed = WbsExecutionTicket::new(
            if field == "operation" {
                WbsIdentifier::new("operation-2".into())?
            } else {
                ticket.operation_id().clone()
            },
            if field == "session" {
                WbsIdentifier::new("session-2".into())?
            } else {
                ticket.session_id().clone()
            },
            if field == "nonce" {
                WbsProof::new("f".repeat(64))?
            } else {
                ticket.nonce().clone()
            },
            if field == "payload" {
                WbsProof::new("b".repeat(64))?
            } else {
                ticket.payload_digest().clone()
            },
            *ticket.expires_at() + chrono::Duration::seconds(i64::from(field == "expiry")),
            if field == "signature" {
                WbsProof::new("f".repeat(64))?
            } else {
                ticket.signature().clone()
            },
        );
        assert_eq!(
            signer.verify_execution_ticket(&input.auth, &changed),
            Err(WbsAutomationError::InvalidProof),
            "field={field}"
        );
    }
    for field in ["session", "capability"] {
        let foreign = fixture(true, field)?;
        assert_eq!(
            signer.verify_execution_ticket(&foreign.auth, &ticket),
            Err(WbsAutomationError::InvalidProof),
            "authorization={field}"
        );
        // Matching the foreign session ID is insufficient without its proof.
        let relabelled = WbsExecutionTicket::new(
            ticket.operation_id().clone(),
            foreign.auth.session_id().clone(),
            ticket.nonce().clone(),
            ticket.payload_digest().clone(),
            *ticket.expires_at(),
            ticket.signature().clone(),
        );
        assert_eq!(
            signer.verify_execution_ticket(&foreign.auth, &relabelled),
            Err(WbsAutomationError::InvalidProof)
        );
    }
    Ok(())
}

#[test]
fn execution_ticket_separates_keys_candidate_proofs_and_operation_nonces() -> TestResult {
    let signer = signer(7)?;
    let input = fixture(true, "")?;
    let ticket = execution_ticket_fixture(&signer, &input)?;
    let other_key = WbsProofSigner::new(Zeroizing::new(vec![8; 32]))?;
    assert_eq!(
        other_key.verify_execution_ticket(&input.auth, &ticket),
        Err(WbsAutomationError::InvalidProof)
    );
    let candidate_proof = prove(&signer, &input)?;
    let crossed = WbsExecutionTicket::new(
        ticket.operation_id().clone(),
        ticket.session_id().clone(),
        ticket.nonce().clone(),
        ticket.payload_digest().clone(),
        *ticket.expires_at(),
        candidate_proof,
    );
    assert_eq!(
        signer.verify_execution_ticket(&input.auth, &crossed),
        Err(WbsAutomationError::InvalidProof)
    );
    assert_eq!(
        verify(&signer, &input, ticket.signature()),
        Err(WbsAutomationError::InvalidProof)
    );
    let changed_operation = signer.execution_ticket(
        &input.auth,
        WbsIdentifier::new("operation-2".into())?,
        ticket.payload_digest().clone(),
        *ticket.expires_at(),
    )?;
    assert_ne!(changed_operation.nonce(), ticket.nonce());
    for field in ["session", "capability"] {
        let foreign = fixture(true, field)?;
        let issued = execution_ticket_fixture(&signer, &foreign)?;
        assert_ne!(issued.nonce(), ticket.nonce(), "authorization={field}");
        assert_eq!(
            signer.verify_execution_ticket(&foreign.auth, &issued),
            Ok(())
        );
    }
    Ok(())
}

#[test]
fn native_payload_binds_each_selected_candidate_target_source_and_request() -> TestResult {
    let signer = signer(7)?;
    for local in [false, true] {
        let original = fixture(local, "")?;
        let scope = proof_scope(&original, "")?;
        let request = WbsCellApplyRequest::new(
            "request-1".into(),
            original.target.clone(),
            original.candidate.display_name().into(),
        )?;
        let payload = signer.native_payload(
            &original.auth,
            &scope,
            &request,
            &original.board,
            &original.candidate,
        )?;
        let mut fields = vec![
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
            "reason",
            "algorithm",
            "algorithm_version",
            "algorithm_as_of",
            "synthetic",
            "provenance",
            "scope",
            "registration",
            "label",
            "extra_context",
        ];
        fields.extend(if local {
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
            ]
        } else {
            vec![
                "organization",
                "wbs_version",
                "wbs_hash",
                "approval",
                "expected_version_absent",
                "origin",
            ]
        });
        for field in fields {
            let changed = fixture(local, field)?;
            let request = WbsCellApplyRequest::new(
                "request-1".into(),
                changed.target.clone(),
                changed.candidate.display_name().into(),
            )?;
            let changed_payload = signer.native_payload(
                &changed.auth,
                &proof_scope(&changed, field)?,
                &request,
                &changed.board,
                &changed.candidate,
            )?;
            assert_ne!(payload, changed_payload, "{field}");
        }
        let other_request = WbsCellApplyRequest::new(
            "request-2".into(),
            original.target.clone(),
            original.candidate.display_name().into(),
        )?;
        assert_ne!(
            payload,
            signer.native_payload(
                &original.auth,
                &scope,
                &other_request,
                &original.board,
                &original.candidate
            )?
        );
        assert_ne!(payload, prove(&signer, &original)?);
    }
    Ok(())
}

#[test]
fn native_payload_refuses_out_of_scope_ineligible_nonmember_and_substituted_text() -> TestResult {
    let signer = signer(7)?;
    for local in [false, true] {
        let input = fixture(local, "")?;
        let scope = proof_scope(&input, "")?;
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
        let other_key_payload = super::signer(8)?.native_payload(
            &input.auth,
            &scope,
            &request,
            &input.board,
            &input.candidate,
        )?;
        assert_ne!(payload, other_key_payload);
        let wrong = WbsCellApplyRequest::new(
            "request-1".into(),
            input.target.clone(),
            "Unselected assignee".into(),
        )?;
        assert_eq!(
            signer.native_payload(&input.auth, &scope, &wrong, &input.board, &input.candidate),
            Err(WbsAutomationError::InvalidProof)
        );
        let other_row = fixture(local, "item")?;
        assert_eq!(
            signer.native_payload(
                &input.auth,
                &proof_scope(&other_row, "")?,
                &request,
                &input.board,
                &input.candidate
            ),
            Err(WbsAutomationError::InvalidProof)
        );
        assert_eq!(
            signer.native_payload(
                &input.auth,
                &scope,
                &request,
                &other_row.board,
                &other_row.candidate
            ),
            Err(WbsAutomationError::InvalidProof)
        );
        let other_candidate = fixture(local, "candidate_id")?;
        assert_eq!(
            signer.native_payload(
                &input.auth,
                &scope,
                &request,
                &input.board,
                &other_candidate.candidate
            ),
            Err(WbsAutomationError::InvalidProof)
        );
        let ineligible = fixture(local, "eligible")?;
        assert_eq!(
            signer.native_payload(
                &input.auth,
                &scope,
                &request,
                &ineligible.board,
                &ineligible.candidate
            ),
            Err(WbsAutomationError::InvalidProof)
        );
    }
    Ok(())
}

mod modal_admission {
    use super::super::authorization::modal_confirmation::modal_fixture;
    use super::*;
    use maekon_core::ports::automation::AutomationPort;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    #[tokio::test]
    async fn modal_gate_rejects_missing_callback_and_expired_access_before_prompt() -> TestResult {
        let (f, _, access, _) = modal_fixture(true, false, None).await?;
        assert_eq!(
            access
                .confirm_cell_apply(&WbsIdentifier::new("missing".into())?)
                .await,
            Err(WbsAutomationError::ConfirmationDenied)
        );
        assert!(f.controller.list_pending_confirmations().await?.is_empty());
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let (f, _, mut access, _) = modal_fixture(
            true,
            true,
            Some(Arc::new(move |value| {
                sender
                    .send(value)
                    .expect("Synthetic modal receiver remains alive");
            })),
        )
        .await?;
        access.pin.until = Instant::now() - Duration::from_secs(1);
        assert_eq!(
            access
                .confirm_cell_apply(&WbsIdentifier::new("expired".into())?)
                .await,
            Err(WbsAutomationError::Expired)
        );
        assert!(matches!(
            receiver.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));
        assert!(f.controller.list_pending_confirmations().await?.is_empty());
        Ok(())
    }
}
