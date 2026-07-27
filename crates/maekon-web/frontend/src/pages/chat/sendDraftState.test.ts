import { describe, expect, it } from 'vitest'
import { clearAcceptedAttachments, clearAcceptedText, removeOptimisticMessage } from './sendDraftState'
import type { ChatMessage } from './types'

describe('chat draft reconciliation', () => {
  it('clears only the text that the backend accepted', () => {
    expect(clearAcceptedText('submitted draft', 'submitted draft')).toBe('')
    expect(clearAcceptedText('new draft typed while sending', 'submitted draft')).toBe('new draft typed while sending')
  })

  it('clears only the attachment list that the backend accepted', () => {
    const submitted = [{ name: 'submitted.txt' }]
    const newer = [...submitted, { name: 'newer.txt' }]

    expect(clearAcceptedAttachments(submitted, submitted)).toEqual([])
    expect(clearAcceptedAttachments(newer, submitted)).toBe(newer)
  })

  it('removes only the failed optimistic message by reference', () => {
    const persisted: ChatMessage = {
      role: 'user',
      content: 'same text',
      timestamp: '2026-07-14T00:00:00Z',
    }
    const optimistic: ChatMessage = { ...persisted }
    const assistant: ChatMessage = {
      role: 'assistant',
      content: 'existing',
      timestamp: '2026-07-14T00:00:01Z',
    }

    expect(removeOptimisticMessage([persisted, optimistic, assistant], optimistic)).toEqual([persisted, assistant])
  })
})
