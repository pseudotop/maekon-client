import { screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../__tests__/helpers/render-helpers'
import type { UpdatePhase, UpdateStatus } from '../api/client'
import i18n from '../i18n'
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
  beforeEach(async () => {
    await i18n.changeLanguage('en')
    mockFetchUpdateStatus.mockReset()
    mockUseUpdateStream.mockReset()
    mockUseUpdateStream.mockReturnValue({
      status: 'connected',
      latest: undefined,
      recoveredAt: null,
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

  it('announces one neutral recovery status without rendering a duplicate warning', async () => {
    mockUseUpdateStream.mockReturnValue({
      status: 'connected',
      latest: undefined,
      recoveredAt: Date.now(),
      lastError: null,
      retryCount: 0,
    })
    mockFetchUpdateStatus.mockResolvedValue(status('Updated', new Date().toISOString()))

    renderWithProviders(<UpdatePanel />)

    const recovery = await screen.findByText(/live update stream connection was restored/i)
    expect(recovery).toHaveAttribute('role', 'status')
    expect(screen.queryByText(/temporary issues/i)).not.toBeInTheDocument()
    expect(screen.getAllByRole('status')).toHaveLength(1)
  })

  it('shows Korean recovery copy and keeps raw updater failures collapsed as technical details', async () => {
    const failed = status('Error', new Date().toISOString())
    failed.message = 'Failed to check for updates: Failed to parse API response: 404 Not Found'
    mockFetchUpdateStatus.mockResolvedValue(failed)
    await i18n.changeLanguage('ko')

    renderWithProviders(<UpdatePanel />)

    await waitFor(() => {
      expect(
        screen.getByText('업데이트 정보를 가져오지 못했습니다. 네트워크 또는 서버 상태를 확인한 뒤 다시 시도하세요.'),
      ).toBeInTheDocument()
    })

    const details = screen.getByTestId('update-status-details')
    expect(details).not.toHaveAttribute('open')
    expect(screen.getByText('기술 세부 정보')).toBeInTheDocument()
    expect(screen.getByText(failed.message)).toBeInTheDocument()
  })
})
