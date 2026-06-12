import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

describe('api-base', () => {
  beforeEach(() => {
    const testWindow = window as Window &
      typeof globalThis & {
        __TAURI_INTERNALS__?: unknown
        __MAEKON_WEB_PORT__?: number
      }
    vi.resetModules()
    delete testWindow.__TAURI_INTERNALS__
    delete testWindow.__MAEKON_WEB_PORT__
    delete (globalThis as { isTauri?: boolean }).isTauri
  })

  afterEach(() => {
    vi.doUnmock('@tauri-apps/api/core')
  })

  it('rewrites API paths to the actual Tauri web port', async () => {
    const testWindow = window as Window &
      typeof globalThis & {
        __TAURI_INTERNALS__?: unknown
      }
    testWindow.__TAURI_INTERNALS__ = {}
    vi.doMock('@tauri-apps/api/core', () => ({
      invoke: vi.fn(async () => 10091),
    }))

    const { getResolvedWebPort, resolveApiUrl } = await import('./api-base')

    await expect(resolveApiUrl('/api/metrics')).resolves.toBe('http://127.0.0.1:10091/api/metrics')
    expect(getResolvedWebPort()).toBe(10091)
  })

  it('rewrites API paths when global API injection is disabled in Tauri', async () => {
    ;(globalThis as { isTauri?: boolean }).isTauri = true
    vi.doMock('@tauri-apps/api/core', () => ({
      invoke: vi.fn(async () => 10092),
    }))

    const { getResolvedWebPort, resolveApiUrl } = await import('./api-base')

    await expect(resolveApiUrl('/api/stream')).resolves.toBe('http://127.0.0.1:10092/api/stream')
    expect(getResolvedWebPort()).toBe(10092)
  })

  it('keeps relative API paths outside Tauri', async () => {
    const { resolveApiUrl } = await import('./api-base')

    await expect(resolveApiUrl('/api/metrics')).resolves.toBe('/api/metrics')
  })
})
