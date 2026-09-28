import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { FeatureCapabilitySnapshot, ProviderSurfaceSpec, SecretBackendCapabilities } from '../../api/contracts'
import OAuthConnectionPanel from './OAuthConnectionPanel'

const mocks = vi.hoisted(() => ({
  invalidateReadiness: vi.fn(),
  oauthConnectionStatus: vi.fn(),
  oauthFlowStatus: vi.fn(),
  oauthRevoke: vi.fn(),
  oauthStartFlow: vi.fn(),
}))

vi.mock('react-i18next', () => {
  const t = (key: string) => key
  return { useTranslation: () => ({ t }) }
})

vi.mock('./oauth-panel-support', () => ({ isOAuthPanelAvailable: () => true }))

vi.mock('../../hooks/useAiReadinessSnapshot', () => ({
  invalidateAiReadinessSnapshotCache: mocks.invalidateReadiness,
}))

vi.mock('../../api/client', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../api/client')>()
  return {
    ...actual,
    oauthCancelFlow: vi.fn(),
    oauthConnectionStatus: mocks.oauthConnectionStatus,
    oauthFlowStatus: mocks.oauthFlowStatus,
    oauthRevoke: mocks.oauthRevoke,
    oauthStartFlow: mocks.oauthStartFlow,
  }
})

const oauthSurface = {
  surface_id: 'provider_surface.openai.oauth',
  display_name: 'OpenAI OAuth',
  stability: 'preview',
} as ProviderSurfaceSpec

const featureSnapshot: FeatureCapabilitySnapshot = { features: [] }
const secretBackendCapabilities = {
  oauth_available: true,
  os_secret_store_available: true,
  oauth_provider_ids: ['openai'],
  default_backend_kind: 'keychain',
  byok_backend_kind: 'keychain',
  fallback_backend_kind: 'memory',
} satisfies SecretBackendCapabilities

function connectionStatus(connected: boolean) {
  return {
    provider_id: 'openai',
    connected,
    expires_at: null,
    scopes: [],
    api_base_url: null,
    has_refresh_token: connected,
  }
}

function renderPanel() {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  const invalidateQueries = vi.spyOn(queryClient, 'invalidateQueries')
  render(
    <QueryClientProvider client={queryClient}>
      <OAuthConnectionPanel
        providerId="openai"
        providerName="OpenAI"
        oauthSurface={oauthSurface}
        featureSnapshot={featureSnapshot}
        secretBackendCapabilities={secretBackendCapabilities}
      />
    </QueryClientProvider>,
  )
  return { invalidateQueries }
}

describe('OAuthConnectionPanel readiness invalidation (#11735)', () => {
  beforeEach(() => {
    mocks.invalidateReadiness.mockReset()
    mocks.oauthConnectionStatus.mockReset()
    mocks.oauthFlowStatus.mockReset()
    mocks.oauthRevoke.mockReset()
    mocks.oauthStartFlow.mockReset()
  })

  it('invalidates both readiness caches after disconnect', async () => {
    mocks.oauthConnectionStatus.mockResolvedValue(connectionStatus(true))
    mocks.oauthRevoke.mockResolvedValue(undefined)
    const { invalidateQueries } = renderPanel()

    fireEvent.click(await screen.findByRole('button', { name: 'settingsOAuth.disconnect' }))

    await waitFor(() => expect(mocks.oauthRevoke).toHaveBeenCalledWith('openai'))
    expect(mocks.invalidateReadiness).toHaveBeenCalledOnce()
    expect(invalidateQueries).toHaveBeenCalledWith({ queryKey: ['feature-capabilities'] })
  })

  it('invalidates both readiness caches when an OAuth flow completes', async () => {
    vi.useFakeTimers()
    try {
      mocks.oauthConnectionStatus
        .mockResolvedValueOnce(connectionStatus(false))
        .mockResolvedValueOnce(connectionStatus(true))
      mocks.oauthStartFlow.mockResolvedValue({ flow_id: 'flow-1', auth_url: 'https://example.test/oauth' })
      mocks.oauthFlowStatus.mockResolvedValue({ status: 'completed' })
      const { invalidateQueries } = renderPanel()

      await act(async () => Promise.resolve())
      fireEvent.click(screen.getByRole('button', { name: 'settingsOAuth.connect' }))
      await act(async () => Promise.resolve())
      await act(async () => vi.advanceTimersByTimeAsync(1500))

      expect(mocks.oauthFlowStatus).toHaveBeenCalledWith('flow-1')
      expect(mocks.invalidateReadiness).toHaveBeenCalledOnce()
      expect(invalidateQueries).toHaveBeenCalledWith({ queryKey: ['feature-capabilities'] })
    } finally {
      vi.useRealTimers()
    }
  })
})
