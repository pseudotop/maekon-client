export type WbsAuthorization = { session_id: string; capability: string }
export type WbsConsentOffer = {
  offer_id: string
  nonce: string
  scope_id: string
  notice_version: 'wbs-cell-assignee.v1'
  document_display_label: string
  permitted_operations: Array<'read_context' | 'fetch_candidates' | 'apply_one_literal_assignee'>
  expires_at: string
}
export type WbsSessionView = { authorization: WbsAuthorization; scope_id: string; expires_at: string }
export type WbsCandidate = {
  candidate_id: string
  display_name: string
  rank: number
  eligible: boolean
  rank_reason: string
  candidate_proof: string
}
export type WbsRecommendation = {
  session_id: string
  query_generation: string
  context: WbsContext
  source: WbsSource | null
  synthetic: boolean
  provenance: string[]
  target_hint: { row: number; column: number }
  candidates: WbsCandidate[]
  expires_at: string
}
export type WbsLocalContext = {
  document_registration_id: string
  input_revision: string
  input_hash: string
  roster_revision: string
  roster_hash: string
}
export type WbsContext = {
  source_kind: 'server_board' | 'local_document_roster'
  organization_id: string | null
  wbs_item_id: string
  wbs_version_id: string | null
  local: WbsLocalContext | null
}
export type WbsSource = {
  source_kind: 'server_board' | 'local_document_roster'
  snapshot_id: string
  snapshot_version: string
  snapshot_hash: string
  wbs_version_id: string | null
  wbs_content_hash: string | null
  approval_id: string | null
  local: {
    input: WbsLocalContext
    provider_selection_digest: string
    provider_name: string
    model: string
    prompt_revision: string
  } | null
}
export type WbsTicket = {
  operation_id: string
  session_id: string
  nonce: string
  payload_digest: string
  expires_at: string
  signature: string
}
export type WbsExecution = {
  operation_id: string
  disposition:
    | 'ready'
    | 'in_progress'
    | 'applied_verified'
    | 'cancelled_before_start'
    | 'denied'
    | 'outcome_unknown'
    | 'readback_mismatch'
    | 'audit_completion_failed'
    | 'already_attempted'
  native_outcome:
    | 'not_started'
    | 'written_unverified'
    | 'cancelled_before_start'
    | 'already_attempted'
    | 'outcome_unknown'
    | 'rejected'
  readback: 'not_read' | 'matches' | 'mismatch' | 'unavailable'
  audit: 'not_started' | 'start_accepted' | 'completion_accepted' | 'completion_rejected'
}
export type WbsCancellation = { session_id: string; closed: boolean; operations: WbsExecution[] }
export type WbsCandidateChoice = Pick<WbsCandidate, 'candidate_id' | 'candidate_proof'> & { query_generation: string }
