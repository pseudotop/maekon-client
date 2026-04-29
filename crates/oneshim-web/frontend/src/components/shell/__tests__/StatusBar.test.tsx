import { screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../../__tests__/helpers/render-helpers'
import StatusBar from '../StatusBar'

// Mock useSSE to control connection state
vi.mock('../../../hooks/useSSE', () => ({
  useSSE: vi.fn(() => ({
    status: 'disconnected',
    latestMetrics: null,
    latestFrame: null,
    idleState: null,
    metricsHistory: [],
    connect: vi.fn(),
    disconnect: vi.fn(),
  })),
}))

// Need to import after mock setup to get mock reference
import { useSSE } from '../../../hooks/useSSE'

const mockUseSSE = vi.mocked(useSSE)

describe('StatusBar', () => {
  it('has displayName', () => {
    expect(StatusBar.displayName).toBe('StatusBar')
  })

  it('shows local mode by default when the realtime stream is disconnected', () => {
    renderWithProviders(<StatusBar />)
    expect(screen.getByText(/local mode/i)).toBeInTheDocument()
  })

  it('shows version string', () => {
    renderWithProviders(<StatusBar />)
    expect(screen.getByText('v0.1.0-test')).toBeInTheDocument()
  })

  it('shows "--" for missing metrics', () => {
    renderWithProviders(<StatusBar />)
    const dashes = screen.getAllByText('--')
    expect(dashes).toHaveLength(2) // CPU and RAM
  })

  it('shows "Connected" when status is connected', () => {
    mockUseSSE.mockReturnValue({
      status: 'connected',
      latestMetrics: null,
      latestFrame: null,
      idleState: null,
      metricsHistory: [],
      connect: vi.fn(),
      disconnect: vi.fn(),
    })
    renderWithProviders(<StatusBar />)
    expect(screen.getByText(/connected/i)).toBeInTheDocument()
  })

  it('shows formatted CPU/RAM when metrics available', () => {
    mockUseSSE.mockReturnValue({
      status: 'connected',
      latestMetrics: {
        timestamp: new Date().toISOString(),
        cpu_usage: 45.2,
        memory_percent: 60,
        memory_used: 8_589_934_592, // 8192MB
        memory_total: 16_000_000_000,
      },
      latestFrame: null,
      idleState: null,
      metricsHistory: [],
      connect: vi.fn(),
      disconnect: vi.fn(),
    })
    renderWithProviders(<StatusBar />)
    expect(screen.getByText('45.2%')).toBeInTheDocument()
    expect(screen.getByText('8.0GB')).toBeInTheDocument()
    expect(screen.queryByText('8192MB')).not.toBeInTheDocument()
  })
})
