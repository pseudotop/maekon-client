/** Receipt-only assignment email draft IPC surface (#9627). */

export interface AssignmentEmailRecipient {
  contact_id: string
  address: string
  contact_label: string
  counterparty_id: string
  organization_label: string
}

export interface AssignmentEmailSyntheticProvenance {
  synthetic: boolean
  source_kind: string
  project_id: string
  counterparty_id: string
  notice: string
}

export interface AssignmentEmailDraft {
  draft_id: string
  draft_hash: string
  organization_id: string
  recipient: AssignmentEmailRecipient
  subject: string
  body: string
  assignment_receipt_id: string
  assignment_id: string
  assignment_hash: string
  wbs_item_id: string
  revision: number
  created_at: string
  stale: boolean
  stale_reason?: string | null
  template_id: string
  template_version: string
  template_hash: string
  synthetic_provenance: AssignmentEmailSyntheticProvenance
}

async function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  const { invoke: tauriInvoke } = await import('@tauri-apps/api/core')
  return tauriInvoke<T>(command, args)
}

export function generateAssignmentEmailDraft(assignmentReceiptId: string): Promise<AssignmentEmailDraft> {
  return invoke('generate_assignment_email_draft', { assignmentReceiptId })
}

export function loadAssignmentEmailDraft(draftId: string): Promise<AssignmentEmailDraft> {
  return invoke('load_assignment_email_draft', { draftId })
}

export function regenerateAssignmentEmailDraft(
  draftId: string,
  assignmentReceiptId: string,
): Promise<AssignmentEmailDraft> {
  return invoke('regenerate_assignment_email_draft', { draftId, assignmentReceiptId })
}
