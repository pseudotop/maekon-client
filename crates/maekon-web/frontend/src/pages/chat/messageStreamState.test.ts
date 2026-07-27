import { describe, expect, it } from 'vitest'
import { applyStreamMessage, completeSessionTurn } from './messageStreamState'
import type { ChatMessage, SessionInfo } from './types'

const userMessage: ChatMessage = {
  role: 'user',
  content: 'Reply exactly: MAEKON_QC_CJ_03_05_OK',
  timestamp: '2026-07-14T10:00:00.000Z',
}

describe('chat message stream state', () => {
  it('reconciles an accumulated terminal result without duplicating streamed text', () => {
    const streaming = applyStreamMessage([userMessage], {
      content: 'MAEKON_QC_CJ_03_05_OK',
      done: false,
      kind: 'delta',
      timestamp: '2026-07-14T10:00:01.000Z',
    })
    const completed = applyStreamMessage(streaming, {
      content: 'MAEKON_QC_CJ_03_05_OK',
      done: true,
      kind: 'result',
      extra: { usage: { input_tokens: 8, output_tokens: 1 } },
    })

    expect(completed).toHaveLength(2)
    expect(completed[1]).toMatchObject({
      role: 'assistant',
      content: 'MAEKON_QC_CJ_03_05_OK',
      streaming: false,
      usage: { input_tokens: 8, output_tokens: 1 },
    })
  })

  it('preserves streamed text when the terminal result only carries usage', () => {
    const streaming = applyStreamMessage([userMessage], {
      content: 'partial response',
      done: false,
      kind: 'delta',
    })
    const completed = applyStreamMessage(streaming, {
      content: '',
      done: true,
      kind: 'result',
      extra: { usage: { input_tokens: 3, output_tokens: 2 } },
    })

    expect(completed[1]).toMatchObject({
      content: 'partial response',
      streaming: false,
      usage: { input_tokens: 3, output_tokens: 2 },
    })
  })

  it('creates an assistant message for a final-only provider result', () => {
    const completed = applyStreamMessage([userMessage], {
      content: 'final-only response',
      done: true,
      kind: 'result',
      timestamp: '2026-07-14T10:00:02.000Z',
    })

    expect(completed[1]).toMatchObject({
      role: 'assistant',
      content: 'final-only response',
      streaming: false,
      timestamp: '2026-07-14T10:00:02.000Z',
    })
  })

  it('updates the completed session turn metadata only', () => {
    const sessions: SessionInfo[] = [
      {
        session_id: 'active',
        provider_name: 'ollama',
        model: 'qwen3:8b',
        state: 'active',
        transport: 'local_llm',
        created_at: '2026-07-14T09:00:00.000Z',
        last_active: '2026-07-14T09:00:00.000Z',
        turn_count: 0,
      },
      {
        session_id: 'other',
        provider_name: 'codex',
        model: 'gpt-5.4',
        state: 'idle',
        transport: 'subprocess',
        created_at: '2026-07-14T09:00:00.000Z',
        last_active: '2026-07-14T09:00:00.000Z',
        turn_count: 4,
      },
    ]

    const completed = completeSessionTurn(sessions, 'active', '2026-07-14T10:00:03.000Z')

    expect(completed[0]).toMatchObject({ turn_count: 1, last_active: '2026-07-14T10:00:03.000Z' })
    expect(completed[1]).toEqual(sessions[1])
  })
})
