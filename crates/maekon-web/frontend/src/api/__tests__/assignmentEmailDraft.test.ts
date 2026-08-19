import { beforeEach, describe, expect, it, vi } from 'vitest'
import {
  generateAssignmentEmailDraft,
  loadAssignmentEmailDraft,
  regenerateAssignmentEmailDraft,
} from '../assignmentEmailDraft'

const mockInvoke = vi.fn()

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => mockInvoke(...args),
}))

describe('assignment email draft IPC (#9627)', () => {
  beforeEach(() => {
    mockInvoke.mockReset()
    mockInvoke.mockResolvedValue({ draft_id: 'emd-1' })
  })

  it('exposes receipt/draft identifiers only', async () => {
    await generateAssignmentEmailDraft('ercv-1')
    await loadAssignmentEmailDraft('emd-1')
    await regenerateAssignmentEmailDraft('emd-1', 'ercv-2')

    expect(mockInvoke.mock.calls).toEqual([
      ['generate_assignment_email_draft', { assignmentReceiptId: 'ercv-1' }],
      ['load_assignment_email_draft', { draftId: 'emd-1' }],
      ['regenerate_assignment_email_draft', { draftId: 'emd-1', assignmentReceiptId: 'ercv-2' }],
    ])
    const wire = JSON.stringify(mockInvoke.mock.calls)
    for (const forbidden of ['organizationId', 'actorId', 'recipient', 'subject', 'body', 'provider', 'send']) {
      expect(wire).not.toContain(forbidden)
    }
  })
})
