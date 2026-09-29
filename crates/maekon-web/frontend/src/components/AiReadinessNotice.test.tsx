import { fireEvent, render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import type { AiCapabilityReadiness, FeatureCapabilitySnapshot } from '../api/contracts'
import { AiReadinessNotice } from './AiReadinessNotice'

vi.mock('react-i18next', () => ({
  useTranslation: () => ({ t: (key: string, fallback?: string) => fallback ?? key }),
}))

function readiness(overrides: Partial<AiCapabilityReadiness> = {}): AiCapabilityReadiness {
  return {
    capability_id: 'chat.subprocess',
    status: 'blocked',
    reason_code: 'provider_invocation_unverified',
    action: 'verify_provider_invocation',
    action_copy_key: 'aiReadiness.action.verifyProviderInvocation',
    dimensions: {
      compiled_capability: true,
      selected_access_mode: 'provider_subscription_cli',
      access_mode_compatible: true,
      endpoint_or_profile_configured: true,
      provider_detection: 'detected',
      provider_auth: 'ready',
      provider_invocation: 'unverified',
      model_availability: 'not_required',
      runtime_flag_enabled: true,
      consent: [],
      apply_requirement: 'restart',
      apply_pending: false,
      privacy_gate: 'enforced_at_invocation',
      egress_gate: 'enforced_at_invocation',
      budget_gate: 'enforced_at_invocation',
      audit_gate: 'enforced_at_invocation',
    },
    ...overrides,
  }
}

function snapshot(item?: AiCapabilityReadiness): FeatureCapabilitySnapshot {
  return {
    features: [],
    ai_readiness: item ? { contract_version: 1, capabilities: [item] } : undefined,
  }
}

describe('AiReadinessNotice shared consumer (#11735)', () => {
  it('renders the exact backend reason and localized action route', () => {
    const { container } = render(
      <AiReadinessNotice snapshot={snapshot(readiness())} capabilityIds={['chat.subprocess']} />,
    )

    const item = container.querySelector('li[data-reason-code="provider_invocation_unverified"]')
    expect(item).toHaveAttribute('data-reason-code', 'provider_invocation_unverified')
    expect(screen.getByRole('link')).toHaveAttribute('href', '/settings/ai-automation')
    expect(screen.getByRole('link')).toHaveTextContent('aiReadiness.action.verifyProviderInvocation')
  })

  it('delegates route actions for consumers hosted outside the main router', () => {
    const onAction = vi.fn()
    render(
      <AiReadinessNotice snapshot={snapshot(readiness())} capabilityIds={['chat.subprocess']} onAction={onAction} />,
    )

    fireEvent.click(screen.getByRole('button', { name: 'aiReadiness.action.verifyProviderInvocation' }))

    expect(onAction).toHaveBeenCalledWith('/settings/ai-automation')
  })

  it('hides ready capabilities by default and shows them in Settings overview mode', () => {
    const ready = readiness({
      status: 'ready',
      reason_code: 'ready',
      action: 'none',
      action_copy_key: 'aiReadiness.action.none',
      dimensions: { ...readiness().dimensions, provider_invocation: 'ready' },
    })
    const { rerender } = render(<AiReadinessNotice snapshot={snapshot(ready)} capabilityIds={['chat.subprocess']} />)
    expect(screen.queryByTestId('ai-readiness-notice')).toBeNull()

    rerender(<AiReadinessNotice snapshot={snapshot(ready)} capabilityIds={['chat.subprocess']} showReady />)
    expect(screen.getByTestId('ai-readiness-notice')).toHaveTextContent('ready')
  })

  it('fails closed when a legacy snapshot has no readiness contract', () => {
    const { container } = render(<AiReadinessNotice snapshot={snapshot()} capabilityIds={['daily_narrative']} />)

    expect(container.querySelector('li[data-reason-code="compiled_capability_missing"]')).not.toBeNull()
    expect(screen.queryByRole('link')).toBeNull()
    expect(screen.queryByText('aiReadiness.action.none')).toBeNull()
  })

  it('renders a blocker while the authoritative snapshot is unavailable', () => {
    const { container } = render(<AiReadinessNotice snapshot={undefined} capabilityIds={['ocr.suggestion_analysis']} />)

    expect(container.querySelector('li[data-reason-code="compiled_capability_missing"]')).not.toBeNull()
  })
})
