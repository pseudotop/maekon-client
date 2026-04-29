import { screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../__tests__/helpers/render-helpers'
import type { UpdatePhase, UpdateStatus } from '../api/client'
import UpdatePanel from './UpdatePanel'

const mockFetchUpdateStatus = vi.fn()
const mockUseUpdateStream = vi.fn()

vi.mock('../api/client', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../api/client')>()
  return {
    ...actual,
    fetchUpdateStatus: () => mockFetchUpdateStatus(),
    postUpdateAction: vi.fn(),
  }
})

vi.mock('../hooks/useUpdateStream', () => ({
  useUpdateStream: () => mockUseUpdateStream(),
}))

function status(phase: UpdatePhase, updatedAt: string): UpdateStatus {
  return {
    enabled: true,
    auto_install: false,
    phase,
    message: 'Already on latest version: 0.4.41-rc.1',
    pending: null,
    download_progress: null,
    rollback: null,
    revision: 1,
    updated_at: updatedAt,
  }
}

describe('UpdatePanel', () => {
  beforeEach(() => {
    mockFetchUpdateStatus.mockReset()
    mockUseUpdateStream.mockReset()
    mockUseUpdateStream.mockReturnValue({
      status: 'connected',
      latest: undefined,
      lastError: null,
      retryCount: 0,
    })
  })

  it('does not show approval-blocked copy for stale non-actionable update status', async () => {
    mockFetchUpdateStatus.mockResolvedValue(status('Updated', '2000-01-01T00:00:00.000Z'))

    renderWithProviders(<UpdatePanel />)

    await waitFor(() => {
      expect(screen.getByText('Already on latest version: 0.4.41-rc.1')).toBeInTheDocument()
    })

    expect(screen.queryByText(/approval is temporarily blocked/i)).not.toBeInTheDocument()
  })

  it('keeps approval-blocked copy when a stale actionable update is waiting', async () => {
    mockFetchUpdateStatus.mockResolvedValue(status('PendingApproval', '2000-01-01T00:00:00.000Z'))

    renderWithProviders(<UpdatePanel />)

    await waitFor(() => {
      expect(screen.getByText(/approval is temporarily blocked/i)).toBeInTheDocument()
    })
  })
})
