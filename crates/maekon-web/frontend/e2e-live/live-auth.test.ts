import { describe, expect, it, vi } from 'vitest'
import { requireLiveAuthToken, resolveLiveAuthToken } from './live-auth'

type MockResponse = {
  ok: boolean
  status: number
  json: () => Promise<unknown>
}

function response(value: unknown): MockResponse {
  return { ok: true, status: 200, json: async () => ({ value }) }
}

describe('live E2E local authentication', () => {
  it('uses an explicit token without contacting WebDriver', async () => {
    const fetchImpl = vi.fn()
    const token = await resolveLiveAuthToken({
      env: { MAEKON_LOCAL_AUTH_TOKEN: 'explicit-token-123456' },
      fetchImpl: fetchImpl as unknown as typeof fetch,
    })

    expect(token).toBe('explicit-token-123456')
    expect(fetchImpl).not.toHaveBeenCalled()
  })

  it('resolves the token through bounded Tauri WebDriver IPC and closes the session', async () => {
    const fetchImpl = vi
      .fn()
      .mockResolvedValueOnce(response({ sessionId: 'session-1' }))
      .mockResolvedValueOnce(response(['tracking-panel', 'main']))
      .mockResolvedValueOnce(response(null))
      .mockResolvedValueOnce(response('webdriver-token-123456'))
      .mockResolvedValueOnce(response(null))

    const token = await resolveLiveAuthToken({
      env: { TAURI_WEBDRIVER_PORT: '4555' },
      fetchImpl: fetchImpl as unknown as typeof fetch,
    })

    expect(token).toBe('webdriver-token-123456')
    expect(fetchImpl).toHaveBeenNthCalledWith(1, 'http://127.0.0.1:4555/session', expect.any(Object))
    expect(fetchImpl).toHaveBeenNthCalledWith(5, 'http://127.0.0.1:4555/session/session-1', { method: 'DELETE' })
  })

  it('fails closed without exposing an invalid token value', async () => {
    expect(() => requireLiveAuthToken({ MAEKON_LOCAL_AUTH_TOKEN: 'bad token value' })).toThrow(
      'MAEKON_LOCAL_AUTH_TOKEN has an invalid format',
    )
  })
})
