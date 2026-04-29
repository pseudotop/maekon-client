import { afterEach, describe, expect, it, vi } from 'vitest'

const STANDALONE_STORAGE_KEY = 'oneshim-web-standalone-mode'

describe('standalone detection', () => {
  const originalUrl = window.location.href

  afterEach(() => {
    window.localStorage.clear()
    window.history.replaceState({}, '', originalUrl)
    delete (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__
    vi.resetModules()
  })

  it('forces live mode in Tauri even when standalone storage is enabled', async () => {
    window.localStorage.setItem(STANDALONE_STORAGE_KEY, '1')
    ;(window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {}

    const standalone = await import('../standalone')

    expect(standalone.isStandaloneModeEnabled()).toBe(false)
    expect(window.localStorage.getItem(STANDALONE_STORAGE_KEY)).toBe('0')
  })

  it('still honors explicit standalone query mode outside Tauri', async () => {
    window.history.replaceState({}, '', `${window.location.pathname}?standalone=1`)

    const standalone = await import('../standalone')

    expect(standalone.isStandaloneModeEnabled()).toBe(true)
    expect(window.localStorage.getItem(STANDALONE_STORAGE_KEY)).toBe('1')
  })
})
