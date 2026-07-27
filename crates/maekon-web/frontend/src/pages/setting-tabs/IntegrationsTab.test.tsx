import { screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../__tests__/helpers/render-helpers'
import IntegrationsTab from './IntegrationsTab'

const mockStatus = vi.fn()
const mockAuthStatus = vi.fn()
const mockInbox = vi.fn()
const mockRefresh = vi.fn()
const mockAck = vi.fn()
const mockDismiss = vi.fn()

vi.mock('../../api/client', () => ({
  fetchIntegrationStatus: (...a: unknown[]) => mockStatus(...a),
  fetchIntegrationAuthStatus: (...a: unknown[]) => mockAuthStatus(...a),
  fetchIntegrationInbox: (...a: unknown[]) => mockInbox(...a),
  refreshIntegrationInbox: (...a: unknown[]) => mockRefresh(...a),
  acknowledgeIntegrationPrompt: (...a: unknown[]) => mockAck(...a),
  dismissIntegrationPrompt: (...a: unknown[]) => mockDismiss(...a),
  startIntegrationDeviceAuthorization: vi.fn(),
  pollIntegrationDeviceAuthorization: vi.fn(),
  cancelIntegrationDeviceAuthorization: vi.fn(),
  resetIntegrationAuth: vi.fn(),
}))

const idleAuthStatus = {
  profile_kind: 'device',
  status: 'unauthenticated',
  interactive: true,
  authenticated: false,
}

beforeEach(() => {
  mockStatus.mockReset()
  mockAuthStatus.mockReset().mockResolvedValue(idleAuthStatus)
  mockInbox.mockReset().mockResolvedValue({ schema_version: '1', prompts: [], pending_count: 0 })
  mockRefresh.mockReset().mockResolvedValue({ schema_version: '1', fetched_count: 0 })
  mockAck.mockReset().mockResolvedValue({ schema_version: '1', prompt_id: 'p1', status: 'acknowledged' })
  mockDismiss.mockReset().mockResolvedValue({ schema_version: '1', prompt_id: 'p1', status: 'dismissed' })
})

describe('IntegrationsTab', () => {
  it('shows an honest disabled state when the outbound integration runtime is off', async () => {
    mockStatus.mockResolvedValue({
      schema_version: '1',
      external_access_enabled: false,
      outbound_runtime: { enabled: false },
    })

    renderWithProviders(<IntegrationsTab />)

    expect(await screen.findByText('Integrations are turned off')).toBeInTheDocument()
    // The inbox / device-auth panels must NOT render in the disabled state.
    expect(screen.queryByText('Device authorization')).not.toBeInTheDocument()
  })

  it('renders outbound auth on loopback without enabling inbound external web access', async () => {
    mockStatus.mockResolvedValue({
      schema_version: '1',
      external_access_enabled: false,
      outbound_runtime: { enabled: true },
    })

    renderWithProviders(<IntegrationsTab />)

    expect(await screen.findByText('Device authorization')).toBeInTheDocument()
    expect(await screen.findByRole('button', { name: 'Connect' })).toBeInTheDocument()
    expect(
      await screen.findByText('No prompts yet. Requests from connected systems will appear here.'),
    ).toBeInTheDocument()
  })

  it('lists inbox prompts with acknowledge/dismiss actions', async () => {
    mockStatus.mockResolvedValue({
      schema_version: '1',
      external_access_enabled: false,
      outbound_runtime: { enabled: true },
    })
    mockInbox.mockResolvedValue({
      schema_version: '1',
      pending_count: 1,
      prompts: [
        {
          prompt_id: 'p1',
          category: 'task',
          priority: 'high',
          title: 'Approve invoice',
          body: 'A new invoice needs review.',
          status: 'pending',
          received_at: new Date().toISOString(),
          status_updated_at: new Date().toISOString(),
          source_system: 'erp',
        },
      ],
    })

    renderWithProviders(<IntegrationsTab />)

    expect(await screen.findByText('Approve invoice')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Acknowledge' })).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Dismiss' })).toBeInTheDocument()

    screen.getByRole('button', { name: 'Acknowledge' }).click()
    await waitFor(() => expect(mockAck).toHaveBeenCalledWith('p1'))
  })

  it('renders one failure message when inbox query and refresh both fail', async () => {
    mockStatus.mockResolvedValue({
      schema_version: '1',
      external_access_enabled: false,
      outbound_runtime: { enabled: true },
    })
    mockInbox.mockRejectedValue(new Error('inbox unavailable'))
    mockRefresh.mockRejectedValue(new Error('refresh unavailable'))

    renderWithProviders(<IntegrationsTab />)

    expect(await screen.findByText('Could not load the inbox.', {}, { timeout: 3_000 })).toBeInTheDocument()
    screen.getByRole('button', { name: 'Refresh' }).click()
    await waitFor(() => expect(mockRefresh).toHaveBeenCalledTimes(1))
    expect(screen.getAllByText('Could not load the inbox.')).toHaveLength(1)
    expect(screen.getByRole('button', { name: 'Retry' })).toBeInTheDocument()
  })
})
