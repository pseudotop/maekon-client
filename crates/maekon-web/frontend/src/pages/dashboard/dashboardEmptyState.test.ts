import { describe, expect, it } from 'vitest'
import { resolveDashboardEmptyState } from './dashboardEmptyState'

describe('resolveDashboardEmptyState', () => {
  it('never claims capture is active before desktop status is known', () => {
    expect(resolveDashboardEmptyState(null)).toBe('unavailable')
  })

  it('surfaces consent-required before pause state', () => {
    expect(
      resolveDashboardEmptyState({
        paused: true,
        indicator_visible: true,
        consent_granted: false,
        permitted: false,
      }),
    ).toBe('consentRequired')
  })

  it.each([
    { paused: true, permitted: true },
    { paused: false, permitted: false },
  ])('reports paused when capture cannot run: %o', ({ paused, permitted }) => {
    expect(
      resolveDashboardEmptyState({
        paused,
        indicator_visible: true,
        consent_granted: true,
        permitted,
      }),
    ).toBe('paused')
  })

  it('reports active only when consent, permission, and pause gates allow capture', () => {
    expect(
      resolveDashboardEmptyState({
        paused: false,
        indicator_visible: true,
        consent_granted: true,
        permitted: true,
      }),
    ).toBe('active')
  })
})
