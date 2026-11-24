import type { CaptureStatus } from '../../hooks/useCaptureStatus'

export type DashboardEmptyState = 'active' | 'paused' | 'consentRequired' | 'unavailable'

export function resolveDashboardEmptyState(status: CaptureStatus | null): DashboardEmptyState {
  if (!status) return 'unavailable'
  if (!status.consent_granted) return 'consentRequired'
  if (status.paused || !status.permitted) return 'paused'
  return 'active'
}

export type DashboardFirstStepAction = 'reviewConsent' | 'resumeCapture' | 'checkStatus'

export interface DashboardFirstStep {
  action: DashboardFirstStepAction
  path: string
}

/**
 * #13735: the empty first screen names the one thing to do next instead of
 * showing zeros. Consent and the pause switch both live on Privacy → Data;
 * an unknown capture state is diagnosed from Support. While capture is active
 * there is nothing to press — the guidance says to keep working.
 */
export function resolveDashboardFirstStep(state: DashboardEmptyState): DashboardFirstStep | null {
  switch (state) {
    case 'consentRequired':
      return { action: 'reviewConsent', path: '/privacy/data' }
    case 'paused':
      return { action: 'resumeCapture', path: '/privacy/data' }
    case 'unavailable':
      return { action: 'checkStatus', path: '/support' }
    case 'active':
      return null
  }
}
