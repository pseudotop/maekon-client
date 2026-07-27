import type { CaptureStatus } from '../../hooks/useCaptureStatus'

export type DashboardEmptyState = 'active' | 'paused' | 'consentRequired' | 'unavailable'

export function resolveDashboardEmptyState(status: CaptureStatus | null): DashboardEmptyState {
  if (!status) return 'unavailable'
  if (!status.consent_granted) return 'consentRequired'
  if (status.paused || !status.permitted) return 'paused'
  return 'active'
}
