import { describe, expect, it } from 'vitest'

describe('CRT-MK-M030: Standalone fallback detection', () => {
  it('M030: without Tauri context, __TAURI_INTERNALS__ is undefined', () => {
    // In Vitest (jsdom), there's no Tauri context
    expect((globalThis as any).__TAURI_INTERNALS__).toBeUndefined()
  })

  it('M031: standalone mode detection works', () => {
    // Simulate how the app detects standalone mode
    const hasTauri = typeof (globalThis as any).__TAURI_INTERNALS__?.invoke === 'function'
    expect(hasTauri).toBe(false)
  })
})
