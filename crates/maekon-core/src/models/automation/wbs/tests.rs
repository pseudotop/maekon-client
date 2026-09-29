use super::*;

#[test]
fn generation_wire_preserves_every_bit_and_canonical_decimal_syntax() {
    assert_eq!(WbsGeneration::new(0), Err(WbsAutomationError::InvalidInput));
    for value in [1, 2, 4096, 9_007_199_254_740_993, u64::MAX] {
        let generation = WbsGeneration::new(value).expect("positive generation");
        assert_eq!(generation.value(), value);
        let wire = serde_json::to_string(&generation).expect("serialize generation");
        assert_eq!(wire, format!("\"{value}\""));
        assert_eq!(
            serde_json::from_str::<WbsGeneration>(&wire).expect("canonical decimal"),
            generation
        );
    }
    for wire in ["1", "null", "true", "[]", "{}"] {
        let error = serde_json::from_str::<WbsGeneration>(wire)
            .expect_err("generation must use a decimal string");
        assert_eq!(error.classify(), serde_json::error::Category::Data);
        assert!(error.to_string().starts_with("invalid type:"), "{error}");
    }
    for wire in [
        "\"\"",
        "\"0\"",
        "\"01\"",
        "\"-1\"",
        "\"+1\"",
        "\" 1\"",
        "\"1 \"",
        "\"1e2\"",
        "\"1.0\"",
        "\"１２\"",
        "\"100000000000000000000\"",
    ] {
        let error =
            serde_json::from_str::<WbsGeneration>(wire).expect_err("noncanonical decimal syntax");
        assert_eq!(error.classify(), serde_json::error::Category::Data);
        assert!(
            error.to_string().starts_with("invalid WBS generation"),
            "{error}"
        );
    }
    let overflow = serde_json::from_str::<WbsGeneration>("\"18446744073709551616\"")
        .expect_err("twenty-digit value exceeds u64");
    assert_eq!(overflow.classify(), serde_json::error::Category::Data);
    assert!(overflow
        .to_string()
        .starts_with("number too large to fit in target type"));
}

#[test]
fn identifiers_preserve_allowed_ascii_and_enforce_the_byte_boundary() {
    for value in ["a".into(), "AZaz09._:-".into(), "x".repeat(128)] {
        let id = WbsIdentifier::new(value.clone()).expect("bounded identifier");
        assert_eq!(id.as_str(), value);
        let wire = serde_json::to_string(&id).expect("serialize identifier");
        assert_eq!(
            serde_json::from_str::<WbsIdentifier>(&wire).expect("identifier"),
            id
        );
    }
    for value in [
        "".into(),
        "x".repeat(129),
        "a b".into(),
        "a/b".into(),
        "a\\b".into(),
        "a@b".into(),
        "한".into(),
        "a\0b".into(),
        "a\nb".into(),
    ] {
        assert_eq!(
            WbsIdentifier::new(value),
            Err(WbsAutomationError::InvalidInput)
        );
    }
}

#[test]
fn proof_requires_exact_lowercase_hex_without_normalizing_input() {
    for value in ["0123456789abcdef".repeat(4), "0".repeat(64), "f".repeat(64)] {
        let proof = WbsProof::new(value.clone()).expect("lowercase 32-byte proof");
        assert_eq!(proof.as_str(), value);
        let wire = serde_json::to_string(&proof).expect("serialize proof");
        assert_eq!(
            serde_json::from_str::<WbsProof>(&wire).expect("proof"),
            proof
        );
    }
    for value in [
        "".into(),
        "a".repeat(63),
        "a".repeat(65),
        "A".repeat(64),
        "g".repeat(64),
        "z".repeat(64),
        format!("{} ", "a".repeat(63)),
        format!("{}\0", "a".repeat(63)),
    ] {
        assert_eq!(WbsProof::new(value), Err(WbsAutomationError::InvalidInput));
    }
}

#[test]
fn display_text_counts_utf8_bytes_and_rejects_controls_without_trimming() {
    for value in [
        "a".into(),
        "Synthetic assignee".into(),
        "가".repeat(170),
        "x".repeat(512),
    ] {
        let text = WbsDisplayText::new(value.clone()).expect("bounded display text");
        assert_eq!(text.as_str(), value);
        let wire = serde_json::to_string(&text).expect("serialize display text");
        assert_eq!(
            serde_json::from_str::<WbsDisplayText>(&wire).expect("display text"),
            text
        );
    }
    for value in [
        "".into(),
        " ".into(),
        "x".repeat(513),
        "가".repeat(171),
        " leading".into(),
        "trailing ".into(),
        "a\tb".into(),
        "a\nb".into(),
        "a\rb".into(),
        "a\0b".into(),
        "a\u{007f}b".into(),
        "a\u{0085}b".into(),
    ] {
        assert_eq!(
            WbsDisplayText::new(value),
            Err(WbsAutomationError::InvalidInput)
        );
    }
}

#[test]
fn authorization_decoding_rejects_unknown_fields_and_invalid_bounded_values() {
    let session = WbsSessionAuthorization::new(
        WbsIdentifier::new("session-fixture".into()).expect("id"),
        WbsProof::new("a".repeat(64)).expect("proof"),
    );
    assert_eq!(session.session_id().as_str(), "session-fixture");
    assert_eq!(session.capability().as_str(), "a".repeat(64));
    let wire = serde_json::to_value(&session).expect("serialize authorization");
    assert_eq!(
        serde_json::from_value::<WbsSessionAuthorization>(wire.clone()).expect("authorization"),
        session
    );
    for field in ["assignee", "path", "pid", "candidate_proof"] {
        let mut changed = wire.clone();
        changed[field] = serde_json::json!("untrusted");
        let error = serde_json::from_value::<WbsSessionAuthorization>(changed)
            .expect_err("unknown authority field");
        assert!(error.to_string().starts_with("unknown field"), "{error}");
    }
    for (field, value, expected) in [
        (
            "session_id",
            serde_json::json!("../document"),
            "wbs_invalid_input",
        ),
        (
            "capability",
            serde_json::json!("A".repeat(64)),
            "wbs_invalid_input",
        ),
        (
            "capability",
            serde_json::Value::Null,
            "invalid type: null, expected a string",
        ),
    ] {
        let mut changed = wire.clone();
        changed[field] = value;
        let error = serde_json::from_value::<WbsSessionAuthorization>(changed)
            .expect_err("invalid authorization field");
        assert_eq!(error.classify(), serde_json::error::Category::Data);
        assert_eq!(error.to_string(), expected);
    }
    let mut missing = wire;
    missing
        .as_object_mut()
        .expect("object")
        .remove("capability");
    let error =
        serde_json::from_value::<WbsSessionAuthorization>(missing).expect_err("missing capability");
    assert_eq!(error.classify(), serde_json::error::Category::Data);
    assert_eq!(error.to_string(), "missing field `capability`");
}

#[test]
fn bounded_messages_hide_their_contents_in_debug_output() {
    let id = WbsIdentifier::new("session-private".into()).expect("id");
    let proof = WbsProof::new("b".repeat(64)).expect("proof");
    let text = WbsDisplayText::new("Private assignee".into()).expect("text");
    assert_eq!(format!("{id:?}"), "WbsIdentifier([REDACTED])");
    assert_eq!(format!("{proof:?}"), "WbsProof([REDACTED])");
    assert_eq!(format!("{text:?}"), "WbsDisplayText([REDACTED])");
    let authorization = WbsSessionAuthorization::new(id, proof);
    assert_eq!(
        format!("{authorization:?}"),
        "WbsSessionAuthorization([REDACTED])"
    );
}

fn scope_test_revision() -> WbsLocalInputRevision {
    WbsLocalInputRevision::new(
        "document-fixture".into(),
        "input-v1".into(),
        "a".repeat(64),
        "roster-v1".into(),
        "b".repeat(64),
    )
    .expect("bounded local revision")
}

fn local_scope_test(
    contexts: Vec<WbsAssignmentContext>,
) -> Result<WbsDocumentScope, WbsAutomationError> {
    WbsDocumentScope::new_local(
        WbsIdentifier::new("scope-fixture".into()).expect("scope identifier"),
        WbsProof::new("c".repeat(64)).expect("registration digest"),
        WbsDisplayText::new("Synthetic workbook".into()).expect("document label"),
        contexts,
    )
}

fn server_scope_test(
    contexts: Vec<WbsAssignmentContext>,
) -> Result<WbsDocumentScope, WbsAutomationError> {
    WbsDocumentScope::new(
        WbsIdentifier::new("server-scope".into()).expect("scope identifier"),
        WbsProof::new("d".repeat(64)).expect("registration digest"),
        WbsProof::new("e".repeat(64)).expect("origin digest"),
        WbsDisplayText::new("Synthetic server workbook".into()).expect("document label"),
        contexts,
    )
}

#[test]
fn local_scope_binds_native_metadata_and_exact_local_revision() {
    let input = scope_test_revision();
    let contexts: Vec<_> = ["row-a", "row-b"]
        .into_iter()
        .map(|row| WbsAssignmentContext::new_local(input.clone(), row.into()).expect("local row"))
        .collect();
    let scope = local_scope_test(contexts.clone()).expect("two local rows");
    assert_eq!(scope.scope_id().as_str(), "scope-fixture");
    assert_eq!(scope.native_registration_digest().as_str(), "c".repeat(64));
    assert_eq!(
        scope.document_display_label().as_str(),
        "Synthetic workbook"
    );
    assert_eq!(
        scope.source_kind(),
        WbsCandidateSourceKind::LocalDocumentRoster
    );
    assert_eq!(scope.server_origin_digest(), None);
    assert_eq!(scope.local_input(), Some(&input));
    assert_eq!(scope.allowed_contexts(), contexts);
    assert_eq!(format!("{scope:?}"), "WbsDocumentScope([REDACTED])");
    assert_eq!(scope.clone().allowed_contexts(), contexts);
}

#[test]
fn local_scope_rejects_ambiguous_rows_and_each_changed_input_pin() {
    let input = scope_test_revision();
    let row = WbsAssignmentContext::new_local(input.clone(), "row-a".into()).expect("local row");
    let server =
        WbsAssignmentContext::new("org-fixture".into(), "row-b".into(), None).expect("server row");
    for contexts in [
        vec![],
        vec![row.clone(), row.clone()],
        vec![row.clone(), server.clone()],
        vec![server, row.clone()],
    ] {
        assert_eq!(
            local_scope_test(contexts).expect_err("ambiguous local scope"),
            WbsAutomationError::InvalidInput
        );
    }
    for (document, revision, input_hash, roster, roster_hash) in [
        ("document-other", "input-v1", "a", "roster-v1", "b"),
        ("document-fixture", "input-v2", "a", "roster-v1", "b"),
        ("document-fixture", "input-v1", "f", "roster-v1", "b"),
        ("document-fixture", "input-v1", "a", "roster-v2", "b"),
        ("document-fixture", "input-v1", "a", "roster-v1", "f"),
    ] {
        let changed = WbsLocalInputRevision::new(
            document.into(),
            revision.into(),
            input_hash.repeat(64),
            roster.into(),
            roster_hash.repeat(64),
        )
        .expect("different valid revision");
        let other =
            WbsAssignmentContext::new_local(changed, "row-b".into()).expect("different row");
        assert_eq!(
            local_scope_test(vec![row.clone(), other]).expect_err("mixed input pins"),
            WbsAutomationError::InvalidInput
        );
    }
    let rows: Vec<_> = (0..65)
        .map(|n| {
            WbsAssignmentContext::new_local(input.clone(), format!("row-{n}"))
                .expect("distinct row")
        })
        .collect();
    assert_eq!(
        local_scope_test(rows[..64].to_vec())
            .expect("64 rows")
            .allowed_contexts()
            .len(),
        64
    );
    assert_eq!(
        local_scope_test(rows).expect_err("65 rows exceed scope"),
        WbsAutomationError::InvalidInput
    );
}

#[test]
fn server_scope_retains_origin_and_rejects_empty_duplicate_mixed_or_oversized_sets() {
    let rows: Vec<_> = (0..65)
        .map(|n| {
            WbsAssignmentContext::new(
                "org-fixture".into(),
                format!("row-{n}"),
                Some("version-fixture".into()),
            )
            .expect("server row")
        })
        .collect();
    let scope = server_scope_test(rows[..64].to_vec()).expect("64 server rows");
    assert_eq!(scope.scope_id().as_str(), "server-scope");
    assert_eq!(scope.native_registration_digest().as_str(), "d".repeat(64));
    assert_eq!(
        scope
            .server_origin_digest()
            .expect("server origin")
            .as_str(),
        "e".repeat(64)
    );
    assert_eq!(
        scope.document_display_label().as_str(),
        "Synthetic server workbook"
    );
    assert_eq!(scope.source_kind(), WbsCandidateSourceKind::ServerBoard);
    assert_eq!(scope.local_input(), None);
    assert_eq!(scope.allowed_contexts(), &rows[..64]);
    let local = WbsAssignmentContext::new_local(scope_test_revision(), "row-local".into())
        .expect("local row");
    for contexts in [
        vec![],
        vec![rows[0].clone(), rows[0].clone()],
        vec![local.clone()],
        vec![rows[0].clone(), local],
        rows,
    ] {
        assert_eq!(
            server_scope_test(contexts).expect_err("invalid server scope"),
            WbsAutomationError::InvalidInput
        );
    }
}

#[test]
fn context_wire_and_accessors_distinguish_local_and_server_identities() {
    let id = |value: &str| WbsIdentifier::new(value.into()).expect("identifier");
    let input = WbsLocalContextView::new(
        id("document-fixture"),
        id("input-v1"),
        WbsProof::new("a".repeat(64)).expect("input hash"),
        id("roster-v1"),
        WbsProof::new("b".repeat(64)).expect("roster hash"),
    );
    let local = WbsContextView::new_local(id("row-a"), input.clone());
    assert_eq!(
        local.source_kind(),
        WbsCandidateSourceKind::LocalDocumentRoster
    );
    assert_eq!(local.organization_id(), None);
    assert_eq!(local.wbs_version_id(), &None);
    assert_eq!(local.wbs_item_id().as_str(), "row-a");
    assert_eq!(local.local(), Some(&input));
    assert_eq!(
        serde_json::to_value(&local).expect("local wire"),
        serde_json::json!({
            "source_kind": "local_document_roster", "organization_id": null, "wbs_item_id": "row-a", "wbs_version_id": null,
            "local": {"document_registration_id": "document-fixture", "input_revision": "input-v1", "input_hash": "a".repeat(64), "roster_revision": "roster-v1", "roster_hash": "b".repeat(64)}
        })
    );
    assert_eq!(format!("{local:?}"), "WbsContextView([REDACTED])");
    let server = WbsContextView::new(id("org-fixture"), id("row-a"), Some(id("version-fixture")));
    assert_eq!(server.source_kind(), WbsCandidateSourceKind::ServerBoard);
    assert_eq!(
        server.organization_id().expect("organization").as_str(),
        "org-fixture"
    );
    assert_eq!(server.wbs_item_id().as_str(), "row-a");
    assert_eq!(
        server.wbs_version_id().as_ref().expect("version").as_str(),
        "version-fixture"
    );
    assert_eq!(server.local(), None);
    assert_eq!(
        serde_json::to_value(&server).expect("server wire"),
        serde_json::json!({
            "source_kind": "server_board", "organization_id": "org-fixture", "wbs_item_id": "row-a", "wbs_version_id": "version-fixture", "local": null
        })
    );
    let no_version = WbsContextView::new(id("org-fixture"), id("row-a"), None);
    assert_eq!(no_version.wbs_version_id(), &None);
    assert_eq!(
        serde_json::to_value(no_version).expect("optional version")["wbs_version_id"],
        serde_json::Value::Null
    );
}

fn view_test_id(value: &str) -> WbsIdentifier {
    WbsIdentifier::new(value.into()).expect("bounded fixture identifier")
}

fn view_test_input(revision: &str) -> WbsLocalContextView {
    WbsLocalContextView::new(
        view_test_id("document-fixture"),
        view_test_id(revision),
        WbsProof::new("a".repeat(64)).expect("input hash"),
        view_test_id("roster-v1"),
        WbsProof::new("b".repeat(64)).expect("roster hash"),
    )
}

fn view_test_source() -> WbsSourceView {
    WbsSourceView::new_local(
        view_test_id("snapshot-fixture"),
        view_test_id("snapshot-v1"),
        WbsProof::new("c".repeat(64)).expect("snapshot hash"),
        WbsLocalSourceView::new(
            view_test_input("input-v1"),
            WbsProof::new("d".repeat(64)).expect("provider digest"),
            WbsDisplayText::new("Synthetic provider".into()).expect("provider"),
            WbsDisplayText::new("Synthetic model".into()).expect("model"),
            view_test_id("prompt-v1"),
        ),
    )
}

fn view_test_session() -> WbsSessionView {
    WbsSessionView::new(
        WbsSessionAuthorization::new(
            view_test_id("session-fixture"),
            WbsProof::new("e".repeat(64)).expect("capability"),
        ),
        view_test_id("scope-fixture"),
        DateTime::parse_from_rfc3339("2026-09-01T12:00:00Z")
            .expect("fixture expiry")
            .with_timezone(&Utc),
    )
}

fn view_test_card(n: u32) -> WbsCandidateCard {
    WbsCandidateCard::new(
        view_test_id(&format!("candidate-{n}")),
        WbsDisplayText::new("Synthetic assignee".into()).expect("name"),
        n + 1,
        true,
        WbsDisplayText::new("Synthetic reason".into()).expect("reason"),
        WbsProof::new("f".repeat(64)).expect("candidate proof"),
    )
}

#[test]
fn source_views_preserve_provenance_and_distinguish_server_approval() {
    let local = view_test_source();
    assert_eq!(
        local.source_kind(),
        WbsCandidateSourceKind::LocalDocumentRoster
    );
    assert_eq!(local.snapshot_id().as_str(), "snapshot-fixture");
    assert_eq!(local.snapshot_version().as_str(), "snapshot-v1");
    assert_eq!(local.snapshot_hash().as_str(), "c".repeat(64));
    assert_eq!(local.wbs_version_id(), None);
    assert_eq!(local.wbs_content_hash(), None);
    assert_eq!(local.approval_id(), None);
    assert_eq!(
        local.local().expect("local provenance").input(),
        &view_test_input("input-v1")
    );
    let wire = serde_json::to_value(&local).expect("local source wire");
    assert_eq!(wire["source_kind"], "local_document_roster");
    assert_eq!(wire["approval_id"], serde_json::Value::Null);
    assert_eq!(wire["local"]["provider_selection_digest"], "d".repeat(64));
    assert_eq!(wire["local"]["provider_name"], "Synthetic provider");
    assert_eq!(wire["local"]["model"], "Synthetic model");
    assert_eq!(wire["local"]["prompt_revision"], "prompt-v1");
    assert_eq!(format!("{local:?}"), "WbsSourceView([REDACTED])");
    let server = WbsSourceView::new(
        view_test_id("server-snapshot"),
        view_test_id("server-v2"),
        WbsProof::new("1".repeat(64)).expect("snapshot hash"),
        view_test_id("wbs-v2"),
        WbsProof::new("2".repeat(64)).expect("content hash"),
        view_test_id("approval-fixture"),
    );
    assert_eq!(server.source_kind(), WbsCandidateSourceKind::ServerBoard);
    assert_eq!(server.snapshot_id().as_str(), "server-snapshot");
    assert_eq!(server.snapshot_version().as_str(), "server-v2");
    assert_eq!(server.snapshot_hash().as_str(), "1".repeat(64));
    assert_eq!(
        server.wbs_version_id().expect("WBS version").as_str(),
        "wbs-v2"
    );
    assert_eq!(
        server.wbs_content_hash().expect("WBS hash").as_str(),
        "2".repeat(64)
    );
    assert_eq!(
        server.approval_id().expect("approval").as_str(),
        "approval-fixture"
    );
    assert_eq!(server.local(), None);
    assert_eq!(
        serde_json::to_value(server).expect("server wire")["local"],
        serde_json::Value::Null
    );
}

#[test]
fn recommendation_bounds_preserve_generation_expiry_and_independent_native_anchor() {
    let session = view_test_session();
    let generation = WbsGeneration::new(17).expect("generation");
    let context = WbsContextView::new_local(view_test_id("row-a"), view_test_input("input-v1"));
    let source = view_test_source();
    let cards: Vec<_> = (0..10).map(view_test_card).collect();
    let provenance = vec![WbsDisplayText::new("Synthetic evidence".into()).expect("evidence"); 16];
    let view = WbsRecommendationView::new(
        &session,
        generation,
        context.clone(),
        Some(source.clone()),
        (true, provenance),
        WbsTargetHint::new(5, 5),
        cards.clone(),
    )
    .expect("legal maxima");
    assert_eq!(view.query_generation(), generation);
    assert_eq!(view.candidates(), cards.as_slice());
    assert_eq!(view.source(), Some(&source));
    assert_eq!(view.context(), &context);
    assert_eq!(view.session_id(), session.authorization().session_id());
    assert_eq!(view.expires_at(), *session.expires_at());
    assert_eq!(view.anchor(), None);
    let anchor = WbsCellAnchor::new(-120, 240, 180, 32, 144).expect("physical anchor");
    let anchored = view.with_anchor(Some(anchor));
    assert_eq!(anchored.anchor(), Some(anchor));
    let wire = serde_json::to_value(&anchored).expect("recommendation wire");
    assert_eq!(wire.get("anchor"), None);
    assert_eq!(wire["query_generation"], "17");
    assert_eq!(wire["synthetic"], true);
    assert_eq!(wire["provenance"].as_array().expect("evidence").len(), 16);
    assert_eq!(format!("{anchored:?}"), "WbsRecommendationView([REDACTED])");
    assert_eq!(anchored.with_anchor(None).anchor(), None);
    for (candidate_count, provenance_count, with_source) in
        [(11, 0, true), (0, 17, true), (1, 0, false)]
    {
        let rejected = WbsRecommendationView::new(
            &session,
            generation,
            context.clone(),
            with_source.then(|| source.clone()),
            (
                true,
                vec![
                    WbsDisplayText::new("Synthetic evidence".into()).expect("evidence");
                    provenance_count
                ],
            ),
            WbsTargetHint::new(5, 5),
            (0..candidate_count).map(view_test_card).collect(),
        );
        assert_eq!(
            rejected.expect_err("invalid aggregate bound"),
            WbsAutomationError::InvalidInput
        );
    }
    let empty = WbsRecommendationView::new(
        &session,
        generation,
        context,
        None,
        (false, vec![]),
        WbsTargetHint::new(5, 5),
        vec![],
    )
    .expect("empty result");
    assert!(empty.candidates().is_empty());
    assert_eq!(empty.source(), None);
}

#[test]
fn recommendation_rejects_crossed_source_modes_or_local_revisions() {
    let session = view_test_session();
    for context in [
        WbsContextView::new(view_test_id("org-fixture"), view_test_id("row-a"), None),
        WbsContextView::new_local(view_test_id("row-a"), view_test_input("input-v2")),
    ] {
        let result = WbsRecommendationView::new(
            &session,
            WbsGeneration::new(1).expect("generation"),
            context,
            Some(view_test_source()),
            (true, vec![]),
            WbsTargetHint::new(5, 5),
            vec![],
        );
        assert_eq!(
            result.expect_err("crossed source identity"),
            WbsAutomationError::InvalidInput
        );
    }
    let server_source = WbsSourceView::new(
        view_test_id("server-snapshot"),
        view_test_id("server-v2"),
        WbsProof::new("1".repeat(64)).expect("snapshot hash"),
        view_test_id("wbs-v2"),
        WbsProof::new("2".repeat(64)).expect("content hash"),
        view_test_id("approval-fixture"),
    );
    let local_context =
        WbsContextView::new_local(view_test_id("row-a"), view_test_input("input-v1"));
    let reverse = WbsRecommendationView::new(
        &session,
        WbsGeneration::new(1).expect("generation"),
        local_context,
        Some(server_source),
        (true, vec![]),
        WbsTargetHint::new(5, 5),
        vec![],
    );
    assert_eq!(
        reverse.expect_err("server result for local context"),
        WbsAutomationError::InvalidInput
    );
}

#[test]
fn cancellation_bounds_keep_native_uncertainty_readback_and_audit_separate() {
    let operations: Vec<_> = (0..65)
        .map(|n| {
            WbsExecutionView::new(
                view_test_id(&format!("operation-{n}")),
                WbsDisposition::OutcomeUnknown,
                WbsNativeOutcome::OutcomeUnknown,
                WbsReadbackState::Matches,
                WbsAuditState::CompletionRejected,
            )
        })
        .collect();
    let cancelled =
        WbsSessionCancellation::new(view_test_id("session-fixture"), operations[..64].to_vec())
            .expect("64 operations");
    assert_eq!(cancelled.operations(), &operations[..64]);
    assert_eq!(
        format!("{cancelled:?}"),
        "WbsSessionCancellation([REDACTED])"
    );
    let wire = serde_json::to_value(&cancelled).expect("cancellation wire");
    assert_eq!(wire["session_id"], "session-fixture");
    assert_eq!(wire["closed"], true);
    assert_eq!(wire["operations"][0]["disposition"], "outcome_unknown");
    assert_eq!(wire["operations"][0]["native_outcome"], "outcome_unknown");
    assert_eq!(wire["operations"][0]["readback"], "matches");
    assert_eq!(wire["operations"][0]["audit"], "completion_rejected");
    assert_eq!(
        WbsSessionCancellation::new(view_test_id("session-fixture"), operations)
            .expect_err("65 operations"),
        WbsAutomationError::InvalidInput
    );
    assert!(
        WbsSessionCancellation::new(view_test_id("session-fixture"), vec![])
            .expect("no operations")
            .operations()
            .is_empty()
    );
}
