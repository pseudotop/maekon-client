import { describe, expect, it } from 'vitest'
import { buildSendSessionMessageArgs } from './ipcPayloads'

describe('buildSendSessionMessageArgs', () => {
  it('wraps send fields under the Rust command request parameter', () => {
    const request = {
      sessionId: 'session-1',
      message: 'hello',
      attachments: [],
      tools: undefined,
      context: undefined,
      responseFormat: undefined,
    }

    expect(buildSendSessionMessageArgs(request)).toEqual({ request })
  })
})
