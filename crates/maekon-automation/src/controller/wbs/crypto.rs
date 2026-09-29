//! Typed proofs bind document consent, sessions, recommendations and cell payloads.
//!
//! This component owns a zeroizing key. It does not grant session authority,
//! perform consent/policy checks, or call a native adapter. The WBS controller
//! must retain and revalidate those independent conditions before a write.

use std::fmt;

use chrono::{DateTime, Utc};
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use zeroize::Zeroizing;

use maekon_core::consent::ConsentRecord;
use maekon_core::models::automation::wbs::{
    WbsAutomationError, WbsConsentAcceptance, WbsDocumentScope, WbsExecutionTicket, WbsIdentifier,
    WbsProof, WbsSessionAuthorization, WBS_CONSENT_NOTICE, WBS_CONSENT_OPERATIONS,
};
use maekon_core::models::wbs_assignment_candidates::{
    local::WbsLocalInputRevision, WbsAssignmentCandidate, WbsAssignmentCandidates,
    WbsAssignmentContext, WbsCandidateSource, WbsCandidateSourceKind,
};
use maekon_core::models::wbs_cell_apply::{WbsCellApplyRequest, WbsCellTargetSnapshot};

const CONSENT: &str = "maekon:wbs:consent:v1";
const SESSION: &str = "maekon:wbs:session:v1";
const NATIVE: &str = "maekon:wbs:native-payload:v1";
const CANDIDATE: &str = "maekon:wbs:candidate:v1";
const EXECUTE_NONCE: &str = "maekon:wbs:execute-nonce:v1";
const EXECUTE: &str = "maekon:wbs:execute:v1";
type Fields = Vec<(&'static str, String)>;

/// Trusted composition's proof issuer; its key never crosses the IPC boundary.
pub struct WbsProofSigner {
    key: Zeroizing<Vec<u8>>,
}

impl WbsProofSigner {
    pub fn new(key: Zeroizing<Vec<u8>>) -> Result<Self, WbsAutomationError> {
        if !(32..=128).contains(&key.len()) {
            return Err(WbsAutomationError::InvalidInput);
        }
        Ok(Self { key })
    }

    pub fn candidate_proof(
        &self,
        auth: &WbsSessionAuthorization,
        target: &WbsCellTargetSnapshot,
        board: &WbsAssignmentCandidates,
        candidate: &WbsAssignmentCandidate,
    ) -> Result<WbsProof, WbsAutomationError> {
        seal(
            &self.key,
            CANDIDATE,
            &candidate_fields(auth, target, board, candidate)?,
        )
    }

    pub fn verify_candidate(
        &self,
        auth: &WbsSessionAuthorization,
        target: &WbsCellTargetSnapshot,
        board: &WbsAssignmentCandidates,
        candidate: &WbsAssignmentCandidate,
        proof: &WbsProof,
    ) -> Result<(), WbsAutomationError> {
        verify(
            &self.key,
            CANDIDATE,
            &candidate_fields(auth, target, board, candidate)?,
            proof,
        )
    }

    /// Issue a proof over one session's exact operation, payload and expiry.
    /// Authority, expiry enforcement, freshness and single use remain controller checks.
    pub fn execution_ticket(
        &self,
        auth: &WbsSessionAuthorization,
        operation: WbsIdentifier,
        payload: WbsProof,
        expires_at: DateTime<Utc>,
    ) -> Result<WbsExecutionTicket, WbsAutomationError> {
        let nonce = seal(
            &self.key,
            EXECUTE_NONCE,
            &[
                ("operation", operation.as_str().into()),
                ("session", auth.session_id().as_str().into()),
                ("capability", auth.capability().as_str().into()),
            ],
        )?;
        let signature = seal(
            &self.key,
            EXECUTE,
            &ticket_fields(auth, &operation, &nonce, &payload, &expires_at),
        )?;
        Ok(WbsExecutionTicket::new(
            operation,
            auth.session_id().clone(),
            nonce,
            payload,
            expires_at,
            signature,
        ))
    }

    /// Verify the proof without treating it as current execution authority.
    pub fn verify_execution_ticket(
        &self,
        auth: &WbsSessionAuthorization,
        ticket: &WbsExecutionTicket,
    ) -> Result<(), WbsAutomationError> {
        if ticket.session_id() != auth.session_id() {
            return Err(WbsAutomationError::InvalidProof);
        }
        verify(
            &self.key,
            EXECUTE,
            &ticket_fields(
                auth,
                ticket.operation_id(),
                ticket.nonce(),
                ticket.payload_digest(),
                ticket.expires_at(),
            ),
            ticket.signature(),
        )
    }

    /// Bind a trusted offer to its exact scope, consent record and expiry.
    /// The controller must enforce live authority and retain/consume the offer.
    pub fn document_consent_nonce(
        &self,
        scope: &WbsDocumentScope,
        consent: &ConsentRecord,
        offer_id: &WbsIdentifier,
        expires_at: &DateTime<Utc>,
    ) -> Result<WbsProof, WbsAutomationError> {
        seal(
            &self.key,
            CONSENT,
            &consent_fields(scope, consent, offer_id, expires_at)?,
        )
    }

    /// Verify a response to a retained offer; this does not consume that offer.
    /// `expires_at` comes from controller state, never the acceptance request.
    pub fn verify_document_consent(
        &self,
        scope: &WbsDocumentScope,
        consent: &ConsentRecord,
        acceptance: &WbsConsentAcceptance,
        expires_at: &DateTime<Utc>,
    ) -> Result<(), WbsAutomationError> {
        validate_acceptance_scope(scope, acceptance)?;
        verify(
            &self.key,
            CONSENT,
            &consent_fields(scope, consent, acceptance.offer_id(), expires_at)?,
            acceptance.nonce(),
        )
    }

    /// Bind a session to an already verified, atomically consumed acceptance.
    /// Issuance does not verify offer freshness or replace controller authority.
    pub fn session_authorization(
        &self,
        scope: &WbsDocumentScope,
        consent: &ConsentRecord,
        acceptance: &WbsConsentAcceptance,
        session_id: WbsIdentifier,
        expires_at: &DateTime<Utc>,
    ) -> Result<WbsSessionAuthorization, WbsAutomationError> {
        let capability = seal(
            &self.key,
            SESSION,
            &session_fields(scope, consent, acceptance, &session_id, expires_at)?,
        )?;
        Ok(WbsSessionAuthorization::new(session_id, capability))
    }

    /// Authentication remains possible for closing an expired/revoked session.
    /// The retained record and expiry authenticate; current state authorizes.
    pub fn verify_session_authorization(
        &self,
        scope: &WbsDocumentScope,
        consent: &ConsentRecord,
        acceptance: &WbsConsentAcceptance,
        expires_at: &DateTime<Utc>,
        auth: &WbsSessionAuthorization,
    ) -> Result<(), WbsAutomationError> {
        verify(
            &self.key,
            SESSION,
            &session_fields(scope, consent, acceptance, auth.session_id(), expires_at)?,
            auth.capability(),
        )
    }

    /// Bind the selected eligible member and exact request before native dispatch.
    /// The controller still checks live source/authority and consumes the ticket.
    pub fn native_payload(
        &self,
        auth: &WbsSessionAuthorization,
        scope: &WbsDocumentScope,
        request: &WbsCellApplyRequest,
        board: &WbsAssignmentCandidates,
        candidate: &WbsAssignmentCandidate,
    ) -> Result<WbsProof, WbsAutomationError> {
        if !scope
            .allowed_contexts()
            .contains(request.target().identity().context())
            || !candidate.eligible()
            || request.assignee_text() != candidate.display_name()
        {
            return Err(WbsAutomationError::InvalidProof);
        }
        let mut fields = candidate_fields(auth, request.target(), board, candidate)?;
        fields.extend(scope_fields(scope)?);
        fields.extend([
            ("request", request.request_id().into()),
            ("assignee", request.assignee_text().into()),
        ]);
        seal(&self.key, NATIVE, &fields)
    }
}

impl fmt::Debug for WbsProofSigner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("WbsProofSigner([REDACTED])")
    }
}

pub(super) fn encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        result.push(char::from(HEX[usize::from(byte >> 4)]));
        result.push(char::from(HEX[usize::from(byte & 15)]));
    }
    result
}

fn mac(
    secret: &[u8],
    domain: &str,
    fields: &[(&'static str, String)],
) -> Result<Hmac<Sha256>, WbsAutomationError> {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret).map_err(|_| WbsAutomationError::Unavailable)?;
    mac.update(&(domain.len() as u64).to_be_bytes());
    mac.update(domain.as_bytes());
    for (name, value) in fields {
        mac.update(&(name.len() as u64).to_be_bytes());
        mac.update(name.as_bytes());
        mac.update(&(value.len() as u64).to_be_bytes());
        mac.update(value.as_bytes());
    }
    Ok(mac)
}

fn seal(
    secret: &[u8],
    domain: &str,
    fields: &[(&'static str, String)],
) -> Result<WbsProof, WbsAutomationError> {
    WbsProof::new(encode(
        mac(secret, domain, fields)?
            .finalize()
            .into_bytes()
            .as_slice(),
    ))
}

fn verify(
    secret: &[u8],
    domain: &str,
    fields: &[(&'static str, String)],
    signature: &WbsProof,
) -> Result<(), WbsAutomationError> {
    let mut bytes = [0_u8; 32];
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&signature.as_str()[i * 2..i * 2 + 2], 16)
            .map_err(|_| WbsAutomationError::InvalidProof)?;
    }
    mac(secret, domain, fields)?
        .verify_slice(&bytes)
        .map_err(|_| WbsAutomationError::InvalidProof)
}

fn local_input_fields(input: &WbsLocalInputRevision) -> Fields {
    vec![
        ("local_document", input.document_registration_id().into()),
        ("local_input_revision", input.input_revision().into()),
        ("local_input_hash", input.input_hash().into()),
        ("local_roster_revision", input.roster_revision().into()),
        ("local_roster_hash", input.roster_hash().into()),
    ]
}

fn context_fields(context: &WbsAssignmentContext) -> Result<Fields, WbsAutomationError> {
    let mut fields = vec![("item", context.wbs_item_id().into())];
    match context.source_kind() {
        WbsCandidateSourceKind::ServerBoard => {
            fields.extend([
                ("context_source", "server_board".into()),
                (
                    "organization",
                    context
                        .organization_id()
                        .ok_or(WbsAutomationError::InvalidInput)?
                        .into(),
                ),
            ]);
            if let Some(version) = context.expected_wbs_version_id() {
                fields.push(("expected_version", version.into()));
            }
        }
        WbsCandidateSourceKind::LocalDocumentRoster => {
            fields.push(("context_source", "local_document_roster".into()));
            fields.extend(local_input_fields(
                context
                    .local_input()
                    .ok_or(WbsAutomationError::InvalidInput)?,
            ));
        }
    }
    Ok(fields)
}

fn source_fields(board: &WbsAssignmentCandidates) -> Result<Fields, WbsAutomationError> {
    let source = board.source().ok_or(WbsAutomationError::StaleCandidate)?;
    let reference = source.reference();
    let mut fields = vec![
        ("query_generation", board.query().generation().to_string()),
        ("query_limit", board.query().limit().to_string()),
        ("snapshot_id", reference.snapshot_id().into()),
        ("snapshot_version", reference.snapshot_version().into()),
        ("snapshot_hash", reference.snapshot_hash().into()),
    ];
    match source {
        WbsCandidateSource::ServerBoard(snapshot) => fields.extend([
            ("result_source", "server_board".into()),
            ("wbs_version", snapshot.wbs_version_id().into()),
            ("wbs_hash", snapshot.wbs_content_hash().into()),
            ("approval", snapshot.approval_id().into()),
        ]),
        WbsCandidateSource::LocalDocumentRoster(snapshot) => {
            fields.push(("result_source", "local_document_roster".into()));
            fields.extend(local_input_fields(snapshot.input()));
            fields.extend([
                (
                    "provider_selection",
                    snapshot.model().provider_selection_digest().into(),
                ),
                ("provider_name", snapshot.model().provider_name().into()),
                ("model", snapshot.model().model().into()),
                ("prompt_revision", snapshot.model().prompt_revision().into()),
            ]);
        }
    }
    Ok(fields)
}

fn target_fields(target: &WbsCellTargetSnapshot) -> Result<Fields, WbsAutomationError> {
    let identity = target.identity();
    let mut fields = vec![
        ("binding", target.binding().binding_id().into()),
        (
            "binding_generation",
            target.binding().generation().to_string(),
        ),
        ("process", identity.process_id().to_string()),
        ("process_start", identity.process_started_at().to_string()),
        ("workbook", identity.workbook_instance_id().into()),
        ("worksheet", identity.worksheet_id().into()),
        ("row", identity.row().to_string()),
        ("column", identity.column().to_string()),
        ("before", target.before_value().into()),
    ];
    fields.extend(context_fields(identity.context())?);
    Ok(fields)
}

fn candidate_fields(
    auth: &WbsSessionAuthorization,
    target: &WbsCellTargetSnapshot,
    board: &WbsAssignmentCandidates,
    candidate: &WbsAssignmentCandidate,
) -> Result<Fields, WbsAutomationError> {
    if target.identity().context() != board.query().context() {
        return Err(WbsAutomationError::InvalidProof);
    }
    let mut fields = target_fields(target)?;
    fields.extend(source_fields(board)?);
    if !board.candidates().contains(candidate) {
        return Err(WbsAutomationError::InvalidProof);
    }
    fields.extend([
        ("session", auth.session_id().as_str().into()),
        ("capability", auth.capability().as_str().into()),
        ("candidate_id", candidate.identity().candidate_id().into()),
        (
            "candidate_hash",
            candidate.identity().candidate_hash().into(),
        ),
        ("user_id", candidate.identity().user_id().into()),
        ("display_name", candidate.display_name().into()),
        ("rank", candidate.rank().to_string()),
        ("eligible", candidate.eligible().to_string()),
        ("reason", candidate.rank_reason().into()),
        ("algorithm", candidate.algorithm().algorithm_id().into()),
        (
            "algorithm_version",
            candidate.algorithm().algorithm_version().into(),
        ),
        (
            "algorithm_as_of",
            candidate.algorithm().as_of().to_rfc3339(),
        ),
        ("synthetic", board.provenance().synthetic().to_string()),
    ]);
    for source in board.provenance().sources() {
        fields.push(("provenance", source.clone()));
    }
    Ok(fields)
}

fn ticket_fields(
    auth: &WbsSessionAuthorization,
    operation: &WbsIdentifier,
    nonce: &WbsProof,
    payload: &WbsProof,
    expires_at: &DateTime<Utc>,
) -> Fields {
    vec![
        ("operation", operation.as_str().into()),
        ("session", auth.session_id().as_str().into()),
        ("capability", auth.capability().as_str().into()),
        ("nonce", nonce.as_str().into()),
        ("payload", payload.as_str().into()),
        ("expires", expires_at.to_rfc3339()),
    ]
}

fn scope_fields(scope: &WbsDocumentScope) -> Result<Fields, WbsAutomationError> {
    let mut fields = vec![
        ("scope", scope.scope_id().as_str().into()),
        (
            "registration",
            scope.native_registration_digest().as_str().into(),
        ),
        (
            "document_label",
            scope.document_display_label().as_str().into(),
        ),
        ("context_count", scope.allowed_contexts().len().to_string()),
    ];
    match scope.source_kind() {
        WbsCandidateSourceKind::ServerBoard => fields.extend([
            ("scope_source", "server_board".into()),
            (
                "origin",
                scope
                    .server_origin_digest()
                    .ok_or(WbsAutomationError::InvalidInput)?
                    .as_str()
                    .into(),
            ),
        ]),
        WbsCandidateSourceKind::LocalDocumentRoster => {
            fields.push(("scope_source", "local_document_roster".into()));
            fields.extend(local_input_fields(
                scope
                    .local_input()
                    .ok_or(WbsAutomationError::InvalidInput)?,
            ));
        }
    }
    for context in scope.allowed_contexts() {
        fields.extend(context_fields(context)?);
    }
    Ok(fields)
}

fn authority_fields(
    scope: &WbsDocumentScope,
    consent: &ConsentRecord,
) -> Result<Fields, WbsAutomationError> {
    let mut fields = scope_fields(scope)?;
    // JSON preserves None versus Some("") and exact timestamps. Only the
    // consent identity and permissions relevant to this operation are bound.
    let identity = serde_json::to_string(&(
        &consent.consent_id,
        &consent.version,
        consent.granted_at,
        consent.expires_at,
        consent.permissions.process_monitoring,
        consent.revoked_at,
        consent.data_deletion_requested,
        &consent.erasure_nonce,
    ))
    .map_err(|_| WbsAutomationError::InvalidInput)?;
    fields.push(("consent_record", identity));
    Ok(fields)
}

fn consent_fields(
    scope: &WbsDocumentScope,
    consent: &ConsentRecord,
    offer_id: &WbsIdentifier,
    expires_at: &DateTime<Utc>,
) -> Result<Fields, WbsAutomationError> {
    let mut fields = authority_fields(scope, consent)?;
    let operations = serde_json::to_string(&WBS_CONSENT_OPERATIONS)
        .map_err(|_| WbsAutomationError::InvalidInput)?;
    fields.extend([
        ("offer", offer_id.as_str().into()),
        ("notice", WBS_CONSENT_NOTICE.into()),
        ("operations", operations),
        ("expires", expires_at.to_rfc3339()),
    ]);
    Ok(fields)
}

fn validate_acceptance_scope(
    scope: &WbsDocumentScope,
    acceptance: &WbsConsentAcceptance,
) -> Result<(), WbsAutomationError> {
    if acceptance.scope_id() != scope.scope_id()
        || acceptance.notice_version().as_str() != WBS_CONSENT_NOTICE
    {
        return Err(WbsAutomationError::InvalidProof);
    }
    Ok(())
}

fn session_fields(
    scope: &WbsDocumentScope,
    consent: &ConsentRecord,
    acceptance: &WbsConsentAcceptance,
    session_id: &WbsIdentifier,
    expires_at: &DateTime<Utc>,
) -> Result<Fields, WbsAutomationError> {
    validate_acceptance_scope(scope, acceptance)?;
    let mut fields = authority_fields(scope, consent)?;
    fields.extend([
        ("session", session_id.as_str().into()),
        ("offer", acceptance.offer_id().as_str().into()),
        ("nonce", acceptance.nonce().as_str().into()),
        ("notice", acceptance.notice_version().as_str().into()),
        ("expires", expires_at.to_rfc3339()),
    ]);
    Ok(fields)
}
