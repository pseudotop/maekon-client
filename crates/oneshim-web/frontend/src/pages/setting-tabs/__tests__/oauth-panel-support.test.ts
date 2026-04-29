import { describe, expect, it } from 'vitest'
import { isOAuthPanelAvailableForRuntime, isProviderOAuthAccessMode } from '../oauth-panel-support'

describe('isProviderOAuthAccessMode', () => {
  it('accepts legacy and normalized ProviderOAuth values', () => {
    expect(isProviderOAuthAccessMode('ProviderOAuth')).toBe(true)
    expect(isProviderOAuthAccessMode('provider_oauth')).toBe(true)
    expect(isProviderOAuthAccessMode('provideroauth')).toBe(true)
  })

  it('rejects non-oauth access modes', () => {
    expect(isProviderOAuthAccessMode('ProviderApiKey')).toBe(false)
    expect(isProviderOAuthAccessMode('')).toBe(false)
    expect(isProviderOAuthAccessMode(null)).toBe(false)
  })
})

describe('isOAuthPanelAvailableForRuntime', () => {
  it('requires a Tauri runtime and disables standalone mode', () => {
    expect(isOAuthPanelAvailableForRuntime({ isTauri: true, isStandaloneMode: false })).toBe(true)
    expect(isOAuthPanelAvailableForRuntime({ isTauri: false, isStandaloneMode: false })).toBe(false)
    expect(isOAuthPanelAvailableForRuntime({ isTauri: true, isStandaloneMode: true })).toBe(false)
  })
})
