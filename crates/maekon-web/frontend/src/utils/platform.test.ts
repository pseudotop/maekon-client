import { afterEach, describe, expect, it, vi } from 'vitest'

describe('platform runtime detection', () => {
  afterEach(() => {
    delete (globalThis as { isTauri?: boolean }).isTauri
    delete (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__
    vi.resetModules()
  })

  it('detects Tauri when the runtime exposes globalThis.isTauri without global API injection', async () => {
    ;(globalThis as { isTauri?: boolean }).isTauri = true

    const platform = await import('./platform')

    expect(platform.IS_TAURI).toBe(true)
  })
})
