import { beforeEach, describe, expect, it, vi } from 'vitest'

const mockInvoke = vi.fn()
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => mockInvoke(...args),
}))

describe('api-base', () => {
  beforeEach(() => {
    const testWindow = window as Window &
      typeof globalThis & {
        __TAURI_INTERNALS__?: unknown
        __MAEKON_WEB_PORT__?: number
        __MAEKON_LOCAL_AUTH__?: string
      }
    vi.resetModules()
    mockInvoke.mockReset()
    delete testWindow.__TAURI_INTERNALS__
    delete testWindow.__MAEKON_WEB_PORT__
    delete testWindow.__MAEKON_LOCAL_AUTH__
    delete (globalThis as { isTauri?: boolean }).isTauri
  })

  it('rewrites API paths to the actual Tauri web port', async () => {
    const testWindow = window as Window &
      typeof globalThis & {
        __TAURI_INTERNALS__?: unknown
      }
    testWindow.__TAURI_INTERNALS__ = {
      invoke: (...args: unknown[]) => mockInvoke(...args),
    }
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'get_web_port') return 10091
      if (cmd === 'get_local_auth_token') return ''
      throw new Error(`unexpected invoke: ${cmd}`)
    })

    const { getResolvedWebPort, resolveApiUrl } = await import('./api-base')

    await expect(resolveApiUrl('/api/metrics')).resolves.toBe('http://127.0.0.1:10091/api/metrics')
    expect(getResolvedWebPort()).toBe(10091)
  })

  it('rewrites API paths when global API injection is disabled in Tauri', async () => {
    ;(globalThis as { isTauri?: boolean }).isTauri = true
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'get_web_port') return 10092
      if (cmd === 'get_local_auth_token') return ''
      throw new Error(`unexpected invoke: ${cmd}`)
    })

    const { getResolvedWebPort, resolveApiUrl } = await import('./api-base')

    await expect(resolveApiUrl('/api/stream')).resolves.toBe('http://127.0.0.1:10092/api/stream')
    expect(getResolvedWebPort()).toBe(10092)
  })

  it('keeps relative API paths outside Tauri', async () => {
    const { resolveApiUrl } = await import('./api-base')

    await expect(resolveApiUrl('/api/metrics')).resolves.toBe('/api/metrics')
  })

  it('waits for delayed Tauri local-auth token before building fetch headers', async () => {
    const testWindow = window as Window &
      typeof globalThis & {
        __TAURI_INTERNALS__?: unknown
      }

    let releaseToken!: () => void
    const tokenReady = new Promise<void>((resolve) => {
      releaseToken = resolve
    })
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'get_web_port') return 10091
      if (cmd === 'get_local_auth_token') {
        await tokenReady
        return 'delayed-local-auth-token'
      }
      throw new Error(`unexpected invoke: ${cmd}`)
    })
    testWindow.__TAURI_INTERNALS__ = {
      invoke: (...args: unknown[]) => mockInvoke(...args),
    }

    const { withResolvedLocalAuthHeaders } = await import('./api-base')
    const pending = withResolvedLocalAuthHeaders({
      headers: { Accept: 'application/json' },
    })

    releaseToken()
    const init = await pending
    const headers = new Headers(init.headers)
    expect(headers.get('Accept')).toBe('application/json')
    expect(headers.get('X-Local-Auth')).toBe('delayed-local-auth-token')
  })

  it('can force-refresh a stale Tauri local-auth token', async () => {
    const testWindow = window as Window &
      typeof globalThis & {
        __TAURI_INTERNALS__?: unknown
        __MAEKON_LOCAL_AUTH__?: string
      }
    testWindow.__MAEKON_LOCAL_AUTH__ = 'stale-token'

    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'get_web_port') return 10091
      if (cmd === 'get_local_auth_token') return 'fresh-token'
      throw new Error(`unexpected invoke: ${cmd}`)
    })
    testWindow.__TAURI_INTERNALS__ = {
      invoke: (...args: unknown[]) => mockInvoke(...args),
    }

    const { resolveLocalAuthToken, withResolvedLocalAuthHeaders } = await import('./api-base')

    await expect(resolveLocalAuthToken({ forceRefresh: true })).resolves.toBe('fresh-token')
    const init = await withResolvedLocalAuthHeaders()
    const headers = new Headers(init.headers)
    expect(headers.get('X-Local-Auth')).toBe('fresh-token')
    expect(testWindow.__MAEKON_LOCAL_AUTH__).toBe('fresh-token')
  })
})
