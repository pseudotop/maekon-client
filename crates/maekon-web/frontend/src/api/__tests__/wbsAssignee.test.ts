import { beforeEach, describe, expect, it, vi } from 'vitest'
import { cancelWbsSession, confirmWbsCandidate, offerWbsDocumentConsent, readWbsExecution } from '../wbsAssignee'

const invoke = vi.fn()
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...args: unknown[]) => invoke(...args) }))
describe('WBS assignee IPC', () => {
  beforeEach(() => {
    invoke.mockReset()
    invoke.mockResolvedValue({})
  })
  it('uses only the typed narrow commands and proof-bound fields', async () => {
    const auth = { session_id: 'session-1', capability: 'a'.repeat(64) }
    await offerWbsDocumentConsent()
    await confirmWbsCandidate(auth, {
      query_generation: '9007199254740993',
      candidate_id: 'candidate-1',
      candidate_proof: 'b'.repeat(64),
    })
    await readWbsExecution(auth, 'operation-1')
    await cancelWbsSession(auth)
    expect(invoke.mock.calls).toEqual([
      ['wbs_offer_document_consent', undefined],
      [
        'wbs_confirm_candidate',
        {
          auth,
          choice: {
            query_generation: '9007199254740993',
            candidate_id: 'candidate-1',
            candidate_proof: 'b'.repeat(64),
          },
        },
      ],
      ['wbs_read_execution', { auth, operation: { operation_id: 'operation-1' } }],
      ['wbs_cancel_session', { auth }],
    ])
  })
})
