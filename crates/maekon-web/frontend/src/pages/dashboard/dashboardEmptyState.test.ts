import { describe, expect, it } from 'vitest'
import { resolveDashboardEmptyState, resolveDashboardFirstStep } from './dashboardEmptyState'

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

describe('resolveDashboardFirstStep (#13735)', () => {
  it('points consent and pause problems at Privacy → Data, where both controls live', () => {
    expect(resolveDashboardFirstStep('consentRequired')).toEqual({ action: 'reviewConsent', path: '/privacy/data' })
    expect(resolveDashboardFirstStep('paused')).toEqual({ action: 'resumeCapture', path: '/privacy/data' })
  })

  it('points an unknown capture state at Support diagnostics', () => {
    expect(resolveDashboardFirstStep('unavailable')).toEqual({ action: 'checkStatus', path: '/support' })
  })

  it('has nothing to press while capture is running', () => {
    expect(resolveDashboardFirstStep('active')).toBeNull()
  })
})
