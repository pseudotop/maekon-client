import type { AssignmentEmailDraft } from '../../api/assignmentEmailDraft'

const PREFIX = 'maekon.assignment-email-draft.v1:'

export interface LocalDraftEdit {
  draft_id: string
  draft_hash: string
  assignment_hash: string
  revision: number
  counterparty_id: string
  contact_id: string
  recipient_address: string
  subject: string
  body: string
  saved_at: string
}

function key(draftId: string): string {
  return `${PREFIX}${draftId}`
}

export function saveLocalDraft(
  draft: AssignmentEmailDraft,
  subject: string,
  body: string,
  storage: Storage = window.sessionStorage,
): LocalDraftEdit {
  const edit: LocalDraftEdit = {
    draft_id: draft.draft_id,
    draft_hash: draft.draft_hash,
    assignment_hash: draft.assignment_hash,
    revision: draft.revision,
    counterparty_id: draft.recipient.counterparty_id,
    contact_id: draft.recipient.contact_id,
    recipient_address: draft.recipient.address,
    subject,
    body,
    saved_at: new Date().toISOString(),
  }
  storage.setItem(key(draft.draft_id), JSON.stringify(edit))
  return edit
}

export function loadLocalDraft(
  draft: AssignmentEmailDraft,
  storage: Storage = window.sessionStorage,
): LocalDraftEdit | null {
  const raw = storage.getItem(key(draft.draft_id))
  if (!raw) return null
  try {
    const edit = JSON.parse(raw) as Partial<LocalDraftEdit>
    const sameSource =
      edit.draft_id === draft.draft_id &&
      edit.draft_hash === draft.draft_hash &&
      edit.assignment_hash === draft.assignment_hash &&
      edit.revision === draft.revision &&
      edit.counterparty_id === draft.recipient.counterparty_id &&
      edit.contact_id === draft.recipient.contact_id &&
      edit.recipient_address === draft.recipient.address
    if (!sameSource || typeof edit.subject !== 'string' || typeof edit.body !== 'string') {
      storage.removeItem(key(draft.draft_id))
      return null
    }
    return edit as LocalDraftEdit
  } catch {
    storage.removeItem(key(draft.draft_id))
    return null
  }
}

export function clearLocalDraft(draftId: string, storage: Storage = window.sessionStorage): void {
  storage.removeItem(key(draftId))
}
