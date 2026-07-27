import { describe, expect, it } from 'vitest'
import { reconcileLoadedSettings } from './settings-form-reconciliation'

describe('reconcileLoadedSettings', () => {
  it('adopts a refreshed payload when the form still matches the previous baseline', () => {
    const current = { web_port: 10091, backend_kind: 'file' }
    const refreshed = { web_port: 10091, backend_kind: 'unavailable' }

    expect(reconcileLoadedSettings(current, JSON.stringify(current), refreshed)).toBe(refreshed)
  })

  it('preserves a real user edit made after the previous baseline loaded', () => {
    const previous = { web_port: 10090 }
    const currentDraft = { web_port: 10091 }
    const refreshed = { web_port: 10090 }

    expect(reconcileLoadedSettings(currentDraft, JSON.stringify(previous), refreshed)).toBe(currentDraft)
  })
})
