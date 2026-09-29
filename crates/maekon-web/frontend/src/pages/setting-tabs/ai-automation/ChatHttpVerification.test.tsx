import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type {
  AiCapabilityId,
  AiReadinessReasonCode,
  ChatHttpVerificationStatus,
  FeatureCapabilitySnapshot,
} from '../../../api/contracts'
import ChatHttpVerification from './ChatHttpVerification'

const { verifyChatHttpProvider, invalidateAiReadinessSnapshotCache } = vi.hoisted(() => ({
  verifyChatHttpProvider: vi.fn(),
  invalidateAiReadinessSnapshotCache: vi.fn(),
}))

vi.mock('../../../api/client', () => ({ verifyChatHttpProvider }))
vi.mock('../../../hooks/useAiReadinessSnapshot', () => ({ invalidateAiReadinessSnapshotCache }))
vi.mock('react-i18next', () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}))

const VERIFIABLE: readonly AiReadinessReasonCode[] = ['provider_invocation_unverified', 'model_availability_unverified']

function snapshot(capabilityId: AiCapabilityId, reason: AiReadinessReasonCode): FeatureCapabilitySnapshot {
  return {
    features: [],
    ai_readiness: {
      contract_version: 1,
      capabilities: [
        {
          capability_id: capabilityId,
          status: reason === 'ready' ? 'ready' : 'blocked',
          reason_code: reason,
          action: reason === 'ready' ? 'none' : 'verify_provider_invocation',
          action_copy_key: 'aiReadiness.action.verifyProviderInvocation',
          dimensions: {
            compiled_capability: true,
            selected_access_mode: 'provider_api_key',
            access_mode_compatible: true,
            endpoint_or_profile_configured: true,
            provider_detection: 'not_required',
            provider_auth: 'ready',
            provider_invocation: reason === 'ready' ? 'ready' : 'unverified',
            model_availability: reason === 'ready' ? 'available' : 'unverified',
            runtime_flag_enabled: true,
            consent: [{ field: 'full_text_extraction', granted: true }],
            apply_requirement: 'restart',
            apply_pending: false,
            privacy_gate: 'enforced_at_invocation',
            egress_gate: 'enforced_at_invocation',
            budget_gate: 'enforced_at_invocation',
            audit_gate: 'enforced_at_invocation',
          },
        },
      ],
    },
  }
}

function renderPanel(readiness: FeatureCapabilitySnapshot | undefined, hasUnsavedChanges = false) {
  const queryClient = new QueryClient()
  const invalidateQueries = vi.spyOn(queryClient, 'invalidateQueries')
  const panel = (next: FeatureCapabilitySnapshot | undefined) => (
    <QueryClientProvider client={queryClient}>
      <ChatHttpVerification snapshot={next} hasUnsavedChanges={hasUnsavedChanges} />
    </QueryClientProvider>
  )
  const view = render(panel(readiness))
  return {
    invalidateQueries,
    rerender: (next: FeatureCapabilitySnapshot) => view.rerender(panel(next)),
    unmount: view.unmount,
  }
}

describe('ChatHttpVerification (#12530)', () => {
  beforeEach(() => {
    verifyChatHttpProvider.mockReset()
    invalidateAiReadinessSnapshotCache.mockReset()
  })

  it('offers the check only while HTTP Chat is blocked on unverified invocation or model', () => {
    const reasons: AiReadinessReasonCode[] = [
      'ready',
      'consent_required',
      'access_mode_mismatch',
      'endpoint_or_profile_required',
      'provider_auth_required',
      'provider_invocation_unavailable',
      'provider_invocation_unverified',
      'model_unavailable',
      'model_availability_unverified',
      'restart_required',
    ]
    for (const reason of reasons) {
      const { unmount } = renderPanel(snapshot('chat.http_api', reason))
      expect(screen.queryByTestId('settings-chat-http-verify') !== null, reason).toBe(VERIFIABLE.includes(reason))
      unmount()
    }

    // The same blocker on another Chat path, or no snapshot at all, offers nothing.
    for (const other of [snapshot('chat.subprocess', 'provider_invocation_unverified'), undefined]) {
      const { unmount } = renderPanel(other)
      expect(screen.queryByTestId('settings-chat-http-verification')).toBeNull()
      unmount()
    }
  })

  it('runs the command and refreshes readiness when the provider is verified', async () => {
    verifyChatHttpProvider.mockResolvedValue({ status: 'verified' })
    const { invalidateQueries, rerender } = renderPanel(snapshot('chat.http_api', 'provider_invocation_unverified'))

    fireEvent.click(screen.getByTestId('settings-chat-http-verify'))

    const result = await screen.findByTestId('settings-chat-http-verification-result')
    expect(result).toHaveAttribute('data-status', 'verified')
    expect(result).toHaveTextContent('settingsAutomation.chatVerification.result.verified')
    expect(verifyChatHttpProvider).toHaveBeenCalledTimes(1)
    expect(verifyChatHttpProvider).toHaveBeenCalledWith()
    expect(invalidateAiReadinessSnapshotCache).toHaveBeenCalledTimes(1)
    expect(invalidateQueries).toHaveBeenCalledWith({ queryKey: ['feature-capabilities'] })

    // Refreshed readiness is ready: the check disappears, the result stays.
    rerender(snapshot('chat.http_api', 'ready'))
    expect(screen.queryByTestId('settings-chat-http-verify')).toBeNull()
    expect(screen.getByTestId('settings-chat-http-verification-result')).toHaveAttribute('data-status', 'verified')
  })

  it('reports every failure status and leaves readiness alone', async () => {
    const failures: Array<[ChatHttpVerificationStatus, string]> = [
      ['not_configured', 'notConfigured'],
      ['consent_required', 'consentRequired'],
      ['restart_required', 'restartRequired'],
      ['budget_exhausted', 'budgetExhausted'],
      ['provider_error', 'providerError'],
      ['timeout', 'timeout'],
    ]
    for (const [status, copy] of failures) {
      verifyChatHttpProvider.mockResolvedValueOnce({ status })
      const { invalidateQueries, unmount } = renderPanel(snapshot('chat.http_api', 'model_availability_unverified'))

      fireEvent.click(screen.getByTestId('settings-chat-http-verify'))

      await waitFor(() =>
        expect(screen.getByTestId('settings-chat-http-verification-result')).toHaveAttribute('data-status', status),
      )
      expect(screen.getByTestId('settings-chat-http-verification-result')).toHaveTextContent(
        `settingsAutomation.chatVerification.result.${copy}`,
      )
      expect(screen.getByTestId('settings-chat-http-verify')).toBeEnabled()
      expect(invalidateQueries).not.toHaveBeenCalled()
      unmount()
    }
    expect(invalidateAiReadinessSnapshotCache).not.toHaveBeenCalled()
  })

  it('reports a rejected command without refreshing readiness', async () => {
    verifyChatHttpProvider.mockRejectedValue({ code: 'service.unavailable', message: 'session manager not available' })
    const { invalidateQueries } = renderPanel(snapshot('chat.http_api', 'provider_invocation_unverified'))

    fireEvent.click(screen.getByTestId('settings-chat-http-verify'))

    expect(await screen.findByTestId('settings-chat-http-verification-result')).toHaveAttribute('data-status', 'failed')
    expect(invalidateAiReadinessSnapshotCache).not.toHaveBeenCalled()
    expect(invalidateQueries).not.toHaveBeenCalled()
  })

  it('waits for unsaved settings, because the check uses the saved configuration', () => {
    renderPanel(snapshot('chat.http_api', 'provider_invocation_unverified'), true)

    const button = screen.getByTestId('settings-chat-http-verify')
    expect(button).toBeDisabled()
    expect(screen.getByTestId('settings-chat-http-verification-save-first')).toBeInTheDocument()
    fireEvent.click(button)
    expect(verifyChatHttpProvider).not.toHaveBeenCalled()
  })
})
