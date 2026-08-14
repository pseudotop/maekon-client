import { beforeEach, describe, expect, it } from 'vitest'
import type { AssignmentEmailDraft } from '../../../api/assignmentEmailDraft'
import { loadLocalDraft, saveLocalDraft } from '../draftSession'

const draft = {
  draft_id: 'emd-1',
  draft_hash: 'd'.repeat(64),
  assignment_hash: 'a'.repeat(64),
  revision: 1,
  recipient: {
    counterparty_id: 'wd-cp-1',
    contact_id: 'wd-cpc-1',
    address: 'pm@vendor.example',
  },
} as AssignmentEmailDraft

describe('local draft session (#9627)', () => {
  beforeEach(() => sessionStorage.clear())

  it('restores edits only for the exact server source', () => {
    saveLocalDraft(draft, 'Edited subject', 'Edited body')
    expect(loadLocalDraft(draft)).toMatchObject({ subject: 'Edited subject', body: 'Edited body' })

    const changedRelation = {
      ...draft,
      recipient: { ...draft.recipient, contact_id: 'wd-cpc-2' },
    }
    expect(loadLocalDraft(changedRelation)).toBeNull()
  })

  it('drops malformed session data instead of showing it as a draft', () => {
    sessionStorage.setItem(`maekon.assignment-email-draft.v1:${draft.draft_id}`, '{bad json')
    expect(loadLocalDraft(draft)).toBeNull()
  })
})
